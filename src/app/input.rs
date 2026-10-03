//! Keys and the mouse: what captures input, and what each key does where.

use super::*;

/// A popup or input box that takes the keys (see `App::modal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Modal {
    Confirm,
    Adding,
    Settings,
    AddFilter,
    Help,
    Menu,
    Hints,
    ImageSearch,
    Viewer,
    Preview,
    Gallery,
    Links,
    Goto,
    SearchInput,
    Searching,
    Filtering,
}

impl App {
    /// Wheel scrolls, a click selects, a double click opens.
    pub fn on_mouse(&mut self, ev: MouseEvent, now: Instant) {
        http::user_input();
        let down = matches!(ev.kind, MouseEventKind::ScrollDown);
        if matches!(ev.kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp) {
            let key = |c| KeyEvent::from(if c { KeyCode::Down } else { KeyCode::Up });
            match self.modal() {
                Some(Modal::Help | Modal::Menu | Modal::Viewer | Modal::Preview | Modal::Links | Modal::ImageSearch) => self.on_key(key(down)),
                Some(Modal::Hints) => self.hints = None,
                _ if self.tab.view == View::Thread => {
                    if let Some(t) = &mut self.tab.thread {
                        t.scroll_lines(if down { 3 } else { -3 });
                    }
                }
                _ if !self.filtering && !self.searching => self.on_key(key(down)),
                _ => {}
            }
            return;
        }
        let right = ev.kind == MouseEventKind::Down(MouseButton::Right);
        if ev.kind != MouseEventKind::Down(MouseButton::Left) && !right {
            return;
        }
        // A right-click selects what's under it and opens its menu (or closes one that's open).
        if right {
            match self.modal() {
                Some(Modal::Menu) => self.menu = None,
                None => {
                    self.select_at(ev.column, ev.row);
                    self.open_menu();
                }
                _ => {}
            }
            return;
        }
        let pos = ratatui::layout::Position::new(ev.column, ev.row);
        // Popups and inputs that aren't the tab's own stay with it: no switching under them.
        let app_wide = matches!(
            self.modal(),
            Some(
                Modal::Confirm
                    | Modal::Adding
                    | Modal::Settings
                    | Modal::AddFilter
                    | Modal::Help
                    | Modal::Menu
                    | Modal::Hints
                    | Modal::ImageSearch
                    | Modal::Goto
                    | Modal::SearchInput
                    | Modal::Searching
                    | Modal::Filtering
            )
        );
        if let Some(&(_, i)) = self.tab_chips.iter().find(|(r, _)| r.contains(pos)).filter(|_| !app_wide) {
            self.switch_tab(i);
            return;
        }
        match self.modal() {
            Some(Modal::Menu) => return self.on_menu_click(ev.column, ev.row),
            Some(Modal::Hints) => {
                self.hints = None;
                return;
            }
            Some(Modal::Links) => return self.on_links_click(ev.column, ev.row, now),
            Some(Modal::ImageSearch) => return self.on_image_search_click(ev.column, ev.row),
            Some(Modal::Help | Modal::Preview | Modal::AddFilter | Modal::Confirm | Modal::Adding) => {
                // Clicking anywhere closes a popup (a save that asks isn't made, nor a site added).
                self.confirm = None;
                self.adding = None;
                self.show_help = false;
                self.tab.preview = None;
                self.filter_add = None;
                return;
            }
            Some(Modal::Viewer | Modal::Searching | Modal::Filtering) => return,
            Some(Modal::Gallery | Modal::Settings | Modal::Goto | Modal::SearchInput) | None => {}
        }
        let Some(target) = self.click_target(ev.column, ev.row) else { return };
        let double = self.last_click.is_some_and(|(t, i)| i == target && now.duration_since(t) < Duration::from_millis(400));
        self.last_click = if double { None } else { Some((now, target)) };
        if self.modal() == Some(Modal::Gallery) {
            if let Some(g) = &mut self.tab.gallery
                && target < g.files.len()
            {
                g.state.select(Some(target));
                if double {
                    self.view_from_gallery(target);
                }
            }
        } else if self.tab.view == View::Thread {
            // A click on a part focuses it (a double click opens it).
            let part = self.thread_part_at(ev.column, ev.row).and_then(|(_, part)| part);
            if let Some(t) = &mut self.tab.thread {
                t.set_cursor(target);
                if t.focus != part {
                    t.focus = part;
                    t.layout = None;
                }
            }
            if double {
                self.on_key(KeyEvent::from(KeyCode::Enter));
            }
        } else if let Some((p, len)) = self.picker()
            && target < len
        {
            p.state.select(Some(target));
            if double {
                self.enter();
            }
        }
    }

    /// Whether a popup or input is taking the keys (the fuzzer asks).
    #[cfg(test)]
    pub(super) fn modal_open(&self) -> Option<()> {
        self.modal().map(|_| ())
    }

    /// What's capturing input, topmost first (the viewer can be open over the gallery, and
    /// image search over the viewer). It gets every key; clicks go to it or close it.
    fn modal(&self) -> Option<Modal> {
        let open = [
            (self.confirm.is_some(), Modal::Confirm),
            (self.adding.is_some(), Modal::Adding),
            (self.settings_popup.is_some(), Modal::Settings),
            (self.filter_add.is_some(), Modal::AddFilter),
            (self.show_help, Modal::Help),
            (self.menu.is_some(), Modal::Menu),
            (self.hints.is_some(), Modal::Hints),
            (self.image_search_panel.is_some(), Modal::ImageSearch),
            (self.tab.viewer.is_some(), Modal::Viewer),
            (self.tab.preview.is_some(), Modal::Preview),
            (self.tab.gallery.is_some() && self.tab.view == View::Thread, Modal::Gallery),
            (self.tab.links.is_some(), Modal::Links),
            (self.goto.is_some(), Modal::Goto),
            (self.search_input.is_some(), Modal::SearchInput),
            (self.searching, Modal::Searching),
            (self.filtering, Modal::Filtering),
        ];
        open.into_iter().find_map(|(on, modal)| on.then_some(modal))
    }

    /// Select what's at a screen position: a list row, or a post (and its part) in a thread.
    fn select_at(&mut self, col: u16, row: u16) {
        if self.tab.view == View::Thread {
            if let Some((e, part)) = self.thread_part_at(col, row)
                && let Some(t) = &mut self.tab.thread
            {
                t.set_cursor(e);
                t.focus = part;
                t.layout = None;
            }
        } else if let Some(target) = self.click_target(col, row)
            && let Some((p, len)) = self.picker()
            && target < len
        {
            p.state.select(Some(target));
        }
    }

    /// The list row or thread post at a screen position.
    fn click_target(&self, col: u16, row: u16) -> Option<usize> {
        let pos = ratatui::layout::Position::new(col, row);
        match self.hit? {
            Hit::List { area, offset, item_height } if area.contains(pos) => {
                Some(offset + ((row - area.y) / item_height.max(1)) as usize)
            }
            Hit::Settings { area, offset } if area.contains(pos) => settings::rows().get(offset + (row - area.y) as usize)?.ok(),
            Hit::Grid { area, offset, cols, cell } if area.contains(pos) => {
                let c = ((col - area.x) / cell.0) as usize;
                (c < cols).then(|| offset + ((row - area.y) / cell.1) as usize * cols + c)
            }
            Hit::Thread { area } if area.contains(pos) => {
                let t = self.tab.thread.as_ref()?;
                let l = t.layout.as_ref()?;
                let line = t.scroll + (row - area.y) as usize;
                l.line(line).map(|(e, _)| e)
            }
            _ => None,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        http::user_input();
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        // `u` right after adding a filter takes it back; any other key keeps it.
        if let Some(undo) = self.filter_undo.take()
            && self.modal().is_none()
            && key.code == KeyCode::Char('u')
            && key.modifiers == KeyModifiers::NONE
        {
            self.filter_undo = Some(undo);
            self.undo_filter();
            return;
        }
        if let Some(modal) = self.modal() {
            match modal {
                Modal::Confirm => self.on_confirm_key(key),
                Modal::Adding => self.on_adding_key(key),
                Modal::Settings => self.on_settings_popup_key(key),
                Modal::AddFilter => self.on_add_filter_key(key),
                Modal::Menu => self.on_menu_key(key),
                Modal::Hints => self.on_hints_key(key),
                Modal::Help => match key.code {
                    KeyCode::Char('j') | KeyCode::Down => self.help_scroll = self.help_scroll.saturating_add(1),
                    KeyCode::Char('k') | KeyCode::Up => self.help_scroll = self.help_scroll.saturating_sub(1),
                    _ => {
                        self.show_help = false;
                        self.help_scroll = 0;
                    }
                },
                Modal::ImageSearch => self.on_image_search_key(key.code),
                Modal::Viewer => match self.keys.action(Scope::Viewer, &key) {
                    Some(action) => self.act(action),
                    None => self.on_viewer_key(key.code),
                },
                Modal::Preview => self.on_preview_key(key.code),
                Modal::Gallery => self.on_gallery_key(key),
                Modal::Links => self.on_links_key(key.code),
                Modal::Goto => self.on_goto_key(key),
                Modal::SearchInput => self.on_search_input_key(key),
                Modal::Searching => self.on_search_key(key),
                Modal::Filtering => self.on_filter_key(key),
            }
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.tab.view == View::Search && self.keys.keys(Action::NextMatch).contains(&crate::keys::Key::from_event(&key)) {
            if self.more_results() {
                self.load_search_page();
            }
            return;
        }
        if let Some(action) = self.keys.action(self.scope(), &key) {
            self.act(action);
            return;
        }
        match key.code {
            KeyCode::F(5) => self.refresh(),
            // Esc backs out a step: from a focused part to its post first.
            KeyCode::Esc if self.focused().is_some() => {
                if let Some(t) = &mut self.tab.thread {
                    t.focus = None;
                    t.layout = None;
                }
            }
            KeyCode::Esc if self.tab.view == View::Thread && self.tab.thread.as_ref().is_some_and(|t| !t.search.is_empty()) => {
                if let Some(t) = &mut self.tab.thread {
                    t.set_search(String::new());
                }
            }
            // Out of a conversation, to the whole thread.
            KeyCode::Esc if self.tab.view == View::Thread && self.tab.thread.as_ref().is_some_and(|t| t.conversation.is_some()) => {
                if let Some(t) = &mut self.tab.thread {
                    t.leave_conversation();
                }
            }
            KeyCode::Esc => {
                if let Some((p, _)) = self.picker().filter(|(p, _)| !p.filter.is_empty()) {
                    p.filter.clear();
                    p.state.select(Some(0));
                } else {
                    self.back();
                }
            }
            _ if self.tab.view == View::Catalog && self.grid_cols > 0 && self.on_grid_key(key.code) => {}
            KeyCode::Char(c @ '1'..='9') if self.tab.view == View::Sites => self.open_favorite(c as usize - '1' as usize),
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => self.back(),
            _ if self.tab.view == View::Thread => self.on_thread_key(key.code, ctrl),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.enter(),
            code => {
                let Some((p, len)) = self.picker() else { return };
                match code {
                    KeyCode::Char('j') | KeyCode::Down => p.move_by(1, len),
                    KeyCode::Char('k') | KeyCode::Up => p.move_by(-1, len),
                    KeyCode::Char('d') if ctrl => p.move_by(10, len),
                    KeyCode::Char('u') if ctrl => p.move_by(-10, len),
                    KeyCode::PageDown => p.move_by(10, len),
                    KeyCode::PageUp => p.move_by(-10, len),
                    KeyCode::Char('g') | KeyCode::Home => p.state.select(Some(0)),
                    KeyCode::Char('G') | KeyCode::End => p.move_by(isize::MAX / 2, len),
                    _ => {}
                }
                if self.tab.view == View::Search {
                    self.search_moved();
                }
            }
        }
    }

    pub fn scope(&self) -> Scope {
        match self.tab.view {
            View::Sites | View::Boards | View::Settings | View::Search => Scope::Lists,
            View::Catalog => Scope::Catalog,
            View::Thread => Scope::Thread,
            View::Watched | View::History | View::Saved => Scope::Saved,
        }
    }

    /// Run a command as its key would where you are (the menu runs its rows this way).
    pub(super) fn run_action(&mut self, action: Action) {
        match self.modal() {
            // The viewer's own `i` opens its file.
            Some(Modal::Viewer) if action == Action::OpenFile => self.on_viewer_key(KeyCode::Char('i')),
            Some(Modal::Gallery) => self.gallery_action(action),
            _ => self.act(action),
        }
    }

    /// Run a (remappable) command.
    pub(super) fn act(&mut self, action: Action) {
        match action {
            Action::Quit => self.quit = true,
            Action::Help => self.show_help = true,
            Action::Settings => self.open_settings(),
            Action::Search if self.tab.view == View::Settings => {}
            Action::Search if self.tab.view == View::Thread => {
                if let Some(t) = &mut self.tab.thread {
                    t.set_search(String::new());
                    self.searching = true;
                }
            }
            Action::Search => self.filtering = true,
            Action::Reload => self.refresh(),
            Action::Filter => self.open_add_filter(),
            Action::Conversation => self.toggle_conversation(),
            Action::Browser => match self.focused_url() {
                Some((_, url)) => self.open_url(&url),
                None => self.open_in_browser(),
            },
            Action::View => match self.focused() {
                Some(&Part::File(k)) => self.view_file(k),
                _ => self.open_viewer(),
            },
            Action::Menu => self.open_menu(),
            Action::Hints => self.open_hints(),
            Action::NextPart => self.step_part(true),
            Action::PrevPart => self.step_part(false),
            Action::Watch => self.toggle_watch(),
            Action::Remove => self.remove_entry(),
            Action::Goto => self.goto = Some(String::new()),
            Action::Links => self.open_links(),
            Action::Hide => self.toggle_hidden(),
            Action::Mine => self.toggle_mine(),
            Action::Gallery => self.open_gallery(),
            Action::Export => self.ask_to_save(Saving::Page),
            Action::ArchiveSearch => self.start_archive_search(),
            Action::ImageSearch => self.open_image_search(),
            Action::NewTab => self.new_tab(),
            Action::Favorite => self.toggle_favorite(),
            Action::AddSite => self.adding = Some(super::Adding::Typing(String::new())),
            Action::Follow => self.toggle_follow(),
            Action::NextTab => self.cycle_tab(true),
            Action::PrevTab => self.cycle_tab(false),
            Action::CloseTab => self.close_tab(),
            Action::Expand => {
                if let Some(t) = &mut self.tab.thread {
                    match t.toggle_expanded() {
                        Ok(_) => {}
                        Err(msg) => self.info(msg),
                    }
                }
            }
            Action::ShowHidden => self.toggle_show_hidden(),
            Action::Copy => match self.focused_url() {
                Some((what, url)) => self.copy_text(what, url),
                None => self.copy(false),
            },
            Action::CopyLink => self.copy(true),
            Action::Sort => {
                self.tab.catalog_sort = self.tab.catalog_sort.next();
                self.tab.catalog_list.state.select(Some(0));
                // Remembered for this board.
                let key = self.board_key();
                self.store.board_prefs.entry(key).or_default().sort = (self.tab.catalog_sort != Sort::Bump).then_some(self.tab.catalog_sort);
                self.save_now();
                self.info(format!("Sorted by {}", self.tab.catalog_sort.as_str()));
            }
            Action::Compact => self.cycle_layout(),
            Action::Download => self.save_here(),
            Action::DownloadPost => self.download(false),
            Action::DownloadThread => self.ask_to_save(Saving::Files),
            Action::Archive => match self.tab.archive_offer.take() {
                Some(key) => {
                    self.open_key(key);
                    self.tab.return_to = None;
                }
                None => self.info("Nothing to open in an archive"),
            },
            Action::OpenFile
            | Action::Replies
            | Action::JumpBack
            | Action::Unread
            | Action::Preview
            | Action::NextMatch
            | Action::PrevMatch
            | Action::Spoiler
            | Action::AllSpoilers => self.thread_action(action),
        }
    }

    fn on_filter_key(&mut self, key: KeyEvent) {
        let Some((p, _)) = self.picker() else {
            self.filtering = false;
            return;
        };
        match key.code {
            KeyCode::Esc => p.filter.clear(),
            code => edit_text(&mut p.filter, code),
        }
        p.state.select(Some(0));
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
            self.filtering = false;
        }
        self.clamp_list();
    }

    /// Moving in the catalog grid: j/k by rows, h/l by columns (h in the first column goes
    /// back, as in lists). Returns whether the key was one of those.
    fn on_grid_key(&mut self, code: KeyCode) -> bool {
        let cols = self.grid_cols as isize;
        let len = self.visible_catalog().len();
        let cur = self.tab.catalog_list.state.selected().unwrap_or(0) as isize;
        let delta = match code {
            KeyCode::Char('j') | KeyCode::Down => cols,
            KeyCode::Char('k') | KeyCode::Up => -cols,
            KeyCode::Char('l') | KeyCode::Right if (cur + 1) % cols != 0 => 1,
            KeyCode::Char('l') | KeyCode::Right => 0,
            KeyCode::Char('h') | KeyCode::Left if cur % cols != 0 => -1,
            _ => return false,
        };
        // Down from the last full row goes to the last thread.
        if delta == cols && cur + cols >= len as isize && cur / cols < (len as isize - 1) / cols {
            self.tab.catalog_list.state.select(Some(len - 1));
        } else if (0..len as isize).contains(&(cur + delta)) {
            self.tab.catalog_list.state.select(Some((cur + delta) as usize));
        }
        true
    }

    fn on_thread_key(&mut self, code: KeyCode, ctrl: bool) {
        if code == KeyCode::Enter && self.take_saved_offer() {
            return;
        }
        let Some(t) = &mut self.tab.thread else { return };
        let half = (t.viewport / 2).max(1) as isize;
        match code {
            KeyCode::Char('j') | KeyCode::Down => t.step(true),
            KeyCode::Char('k') | KeyCode::Up => t.step(false),
            KeyCode::Char('J') => t.scroll_lines(1),
            KeyCode::Char('K') => t.scroll_lines(-1),
            KeyCode::Char('d') if ctrl => t.scroll_lines(half),
            KeyCode::Char('u') if ctrl => t.scroll_lines(-half),
            KeyCode::PageDown | KeyCode::Char(' ') => t.scroll_lines(half * 2 - 1),
            KeyCode::PageUp => t.scroll_lines(-(half * 2 - 1)),
            KeyCode::Char('g') | KeyCode::Home => t.select_entry(0),
            KeyCode::Char('G') | KeyCode::End => t.select_entry(usize::MAX),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right if t.focus.is_some() => {
                if let Some(part) = t.focus.clone() {
                    self.activate(part);
                }
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                let quotes = t.current().map(|p| p.quotes.clone()).unwrap_or_default();
                if !quotes.into_iter().any(|q| t.jump_to(q)) {
                    self.follow_link();
                }
            }
            _ => {}
        }
    }

    fn thread_action(&mut self, action: Action) {
        let search_key = self.keys.key(Action::Search);
        let Some(t) = &mut self.tab.thread else { return };
        match action {
            Action::Preview => self.open_preview(),
            Action::NextMatch | Action::PrevMatch if t.matches.is_empty() => {
                let msg = if t.search.is_empty() { format!("No search; press {search_key} to search the thread") } else { "No matches".into() };
                self.info(msg);
            }
            Action::NextMatch | Action::PrevMatch => {
                if let Some(i) = t.next_match(action == Action::NextMatch) {
                    t.select(i);
                    let k = t.matches.iter().position(|&m| m == i).unwrap_or(0);
                    let msg = format!("Match {}/{} for \"{}\"", k + 1, t.matches.len(), t.search);
                    self.info(msg);
                }
            }
            Action::Spoiler => {
                let i = t.selected;
                if !t.revealed.remove(&i) {
                    t.revealed.insert(i);
                }
                t.layout = None;
            }
            Action::AllSpoilers => {
                t.reveal_all = !t.reveal_all;
                t.revealed.clear();
                t.layout = None;
                let msg = if t.reveal_all { "Showing all spoilers" } else { "Hiding spoilers" };
                self.info(msg);
            }
            Action::Replies => match t.backlinks.get(t.selected).and_then(|b| b.first()) {
                Some(&no) => {
                    t.jump_to(no);
                }
                None => self.info("No replies to this post"),
            },
            Action::JumpBack => {
                if let Some(i) = t.jumps.pop() {
                    t.select(i);
                } else if let Some((site, board, no, post)) = self.tab.trail.pop() {
                    // Back to the thread we came from by a cross-thread link.
                    self.switch_site(site);
                    self.open_thread_at(board, no, Some(post));
                }
            }
            Action::Unread => match (0..t.posts.len()).find(|&i| t.is_new(i)) {
                Some(i) => {
                    t.jumps.push(t.selected);
                    t.select(i);
                }
                None => self.info("No unread posts"),
            },
            Action::OpenFile => match t.current().and_then(|p| p.files.first()).cloned() {
                Some(f) => self.open_file(&f),
                None => self.info("Post has no file"),
            },
            _ => {}
        }
    }

    fn on_search_key(&mut self, key: KeyEvent) {
        let Some(t) = &mut self.tab.thread else {
            self.searching = false;
            return;
        };
        match key.code {
            KeyCode::Esc => {
                t.set_search(String::new());
                self.searching = false;
            }
            KeyCode::Enter => {
                self.searching = false;
                let msg = match t.next_match(true) {
                    Some(i) => {
                        t.select(i);
                        format!("{} posts match \"{}\" (n/N to move)", t.matches.len(), t.search)
                    }
                    None if t.search.is_empty() => return,
                    None => format!("No posts match \"{}\"", t.search),
                };
                self.info(msg);
            }
            KeyCode::Backspace => {
                let mut q = t.search.clone();
                q.pop();
                t.set_search(q);
            }
            KeyCode::Char(c) => {
                let q = format!("{}{c}", t.search);
                t.set_search(q);
            }
            _ => {}
        }
    }

    fn open_preview(&mut self) {
        let Some(t) = &self.tab.thread else { return };
        let Some(p) = t.current() else { return };
        let posts: Vec<usize> = p.quotes.iter().filter_map(|q| t.index.get(q).copied()).collect();
        let elsewhere: Vec<u64> = p.links.iter().filter_map(|l| l.post).filter(|n| !t.index.contains_key(n)).collect();
        if posts.is_empty() && elsewhere.is_empty() {
            self.info("Post quotes nothing");
            return;
        }
        self.tab.preview = Some(Preview { posts, elsewhere, scroll: 0 });
    }

    fn on_preview_key(&mut self, code: KeyCode) {
        let Some(p) = &mut self.tab.preview else { return };
        match code {
            KeyCode::Char('j') | KeyCode::Down => p.scroll = p.scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => p.scroll = p.scroll.saturating_sub(1),
            KeyCode::Char(' ') | KeyCode::PageDown => p.scroll = p.scroll.saturating_add(10),
            KeyCode::PageUp => p.scroll = p.scroll.saturating_sub(10),
            KeyCode::Char('g') => p.scroll = 0,
            // Jump to the (first) quoted post.
            KeyCode::Enter => {
                let first = p.posts.first().copied();
                self.tab.preview = None;
                if let (Some(i), Some(t)) = (first, &mut self.tab.thread) {
                    t.jumps.push(t.selected);
                    t.select(i);
                }
            }
            _ => self.tab.preview = None,
        }
    }

    fn on_viewer_key(&mut self, code: KeyCode) {
        use crate::images::Crop;
        let Some(v) = &mut self.tab.viewer else { return };
        let n = v.files.len();
        let zoomed = !v.crop.is_fit();
        match code {
            // Zoom: + and - (= is + without shift), 0 fits it again. Zoomed in, the arrows
            // and h/j/k/l move around, page up / down go to the other files, and esc fits.
            KeyCode::Char('+' | '=') => v.crop = v.crop.zoomed(true),
            KeyCode::Char('-') => v.crop = v.crop.zoomed(false),
            KeyCode::Char('0') => v.crop = Crop::FIT,
            KeyCode::Esc if zoomed => v.crop = Crop::FIT,
            KeyCode::Char('h') | KeyCode::Left if zoomed => v.crop = v.crop.moved(-1, 0, v.shown.unwrap_or(v.crop.guess_shown())),
            KeyCode::Char('l') | KeyCode::Right if zoomed => v.crop = v.crop.moved(1, 0, v.shown.unwrap_or(v.crop.guess_shown())),
            KeyCode::Char('k') | KeyCode::Up if zoomed => v.crop = v.crop.moved(0, -1, v.shown.unwrap_or(v.crop.guess_shown())),
            KeyCode::Char('j') | KeyCode::Down if zoomed => v.crop = v.crop.moved(0, 1, v.shown.unwrap_or(v.crop.guess_shown())),
            KeyCode::PageUp => {
                v.index = (v.index + n - 1) % n;
                v.crop = Crop::FIT;
            }
            KeyCode::PageDown => {
                v.index = (v.index + 1) % n;
                v.crop = Crop::FIT;
            }
            KeyCode::Esc | KeyCode::Char('q' | 'v') => {
                // Back in the gallery, on the file last viewed; or in the thread, on its post.
                if let Some(g) = &mut self.tab.gallery {
                    g.state.select(Some(v.index));
                } else if let Some(&no) = v.posts.get(v.index)
                    && let Some(t) = &mut self.tab.thread
                    && let Some(&i) = t.index.get(&no)
                    && i != t.selected
                {
                    t.select(i);
                }
                self.tab.viewer = None;
            }
            KeyCode::Char('h' | 'k') | KeyCode::Left | KeyCode::Up => v.index = (v.index + n - 1) % n,
            // Space pauses an animated GIF; otherwise it's the next file.
            KeyCode::Char(' ') if self.images.toggle_pause(&v.files[v.index].url) => {
                let paused = self.images.is_paused(&v.files[v.index].url);
                self.info(if paused { "Paused (space plays)" } else { "Playing" });
            }
            KeyCode::Char('l' | 'j' | ' ') | KeyCode::Right | KeyCode::Down => v.index = (v.index + 1) % n,
            KeyCode::Char('i') | KeyCode::Enter => {
                let f = v.files[v.index].clone();
                self.open_file(&f);
            }
            _ => {}
        }
    }
}
