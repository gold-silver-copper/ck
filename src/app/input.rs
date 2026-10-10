//! Keys and the mouse: what captures input, and what each key does where.

use super::*;
use ratatui::crossterm::event::MouseEventKind::{Down, ScrollDown, ScrollUp};
use ratatui::layout::Position;

/// A popup or input box that takes the keys (see `App::modal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Modal {
    Confirm,
    Adding,
    Settings,
    AddFilter,
    Help,
    Menu,
    Hints,
    ImageSearch,
    Reply,
    Viewer,
    Preview,
    Gallery,
    Links,
    Goto,
    SearchInput,
    Searching,
    Filtering,
}

/// What a frame showed: the screen (its view, tab and place), how many tabs, and what's on
/// top.
pub(super) type Shown = ((View, usize, bool, bool, u32, CatalogLayout), usize, Option<Modal>);

/// What the mouse does with one gesture.
type Handler = fn(&mut App, MouseEvent, Instant);

impl App {
    /// What the wheel, a left click and a right click do with `top` on top, and whether the
    /// tab chips can be clicked under it. Every popup and input has its row: a new one
    /// can't be added without saying what the mouse does over it.
    #[deny(clippy::wildcard_enum_match_arm)]
    fn mouse(&self, top: Option<Modal>) -> (Handler, Handler, Handler, bool) {
        let Some(top) = top else { return (App::scroll, App::click_view, App::menu_at, true) };
        match top {
            // A click doesn't make a save that asks, nor add a site; the wheel answers nothing.
            Modal::Confirm | Modal::Adding => (App::ignore, App::close_top, App::ignore, false),
            // The key editor waiting for a key: the mouse is none.
            Modal::Settings if matches!(&self.popup, Some(Popup::Settings(SettingsPopup::Keys { capture: Some(_), .. }))) => (App::ignore, App::ignore, App::ignore, false),
            // Settings popups take keys only: a click mustn't reach the row behind (a double
            // click would change that setting).
            Modal::Settings => (App::rows, App::ignore, App::ignore, false),
            Modal::AddFilter | Modal::Help => (App::rows, App::close_top, App::ignore, false),
            // Labels are on what's drawn, which the wheel would move.
            Modal::Hints => (App::close_top, App::close_top, App::ignore, false),
            Modal::Menu => (App::rows, App::on_menu_click, App::close_top, false),
            Modal::ImageSearch => (App::rows, App::on_image_search_click, App::ignore, false),
            // Over the browser view, the mouse is the page's; elsewhere in the box, nothing.
            Modal::Reply => (App::on_reply_wheel, App::on_reply_click, App::ignore, false),
            Modal::Preview => (App::rows, App::close_top, App::ignore, true),
            Modal::Links => (App::rows, App::on_links_click, App::ignore, true),
            // The gallery's wheel moves through its grid, a row a notch.
            Modal::Gallery => (App::rows, App::on_gallery_click, App::ignore, true),
            Modal::Viewer => (App::rows, App::ignore, App::ignore, true),
            // Typing stays open while the thread is read, and its posts clicked.
            Modal::Goto | Modal::SearchInput => (App::scroll_thread, App::click_view, App::ignore, false),
            Modal::Searching | Modal::Filtering => (App::scroll_thread, App::ignore, App::ignore, false),
        }
    }

    /// The mouse, to what's on top. A click lands only on the frame it was aimed at: once
    /// something may have moved (a wheel notch among them), it does nothing until the next.
    pub fn on_mouse(&mut self, ev: MouseEvent, now: Instant) {
        http::user_input();
        let (before, top) = (self.shown(), self.modal());
        let (wheel, left, right, tabs) = self.mouse(top);
        let pos = Position::new(ev.column, ev.row);
        let fresh = self.drawn.fresh && self.drawn.shown == Some(before);
        let clicked = match ev.kind {
            ScrollDown | ScrollUp => {
                wheel(self, ev, now);
                false
            }
            Down(MouseButton::Right) if fresh => {
                right(self, ev, now);
                true
            }
            Down(MouseButton::Left) if fresh => {
                // `u` takes back a filter only right after it's added.
                self.filter_undo = None;
                match self.drawn().and_then(|d| d.tabs.iter().find(|(r, _)| r.contains(pos)).map(|&(_, i)| i)).filter(|_| tabs) {
                    Some(i) => self.switch_tab(i),
                    None => left(self, ev, now),
                }
                true
            }
            _ => return,
        };
        if !clicked || self.shown() != before {
            self.forget_frame();
        }
    }

    fn ignore(&mut self, _: MouseEvent, _: Instant) {}

    /// Close what's on top (a question over the preview, not the preview too).
    fn close_top(&mut self, _: MouseEvent, _: Instant) {
        if self.popup.take().is_none() {
            self.tab.popup = None;
        }
    }

    /// The wheel moves a popup's rows as the arrow keys do.
    fn rows(&mut self, ev: MouseEvent, _: Instant) {
        if let Some(m) = self.modal() {
            self.on_modal_key(m, arrow(ev));
        }
    }

    /// The wheel scrolls a thread three lines a notch.
    fn scroll_thread(&mut self, ev: MouseEvent, _: Instant) {
        let thread = self.tab.view() == View::Thread;
        if let Some(t) = self.tab.thread.as_mut().filter(|_| thread) {
            t.scroll_lines(if ev.kind == ScrollDown { 3 } else { -3 });
        }
    }

    /// The wheel scrolls a thread, and moves through a list.
    fn scroll(&mut self, ev: MouseEvent, now: Instant) {
        if self.tab.view() == View::Thread { self.scroll_thread(ev, now) } else { self.on_view_key(arrow(ev)) }
    }

    /// A right-click selects what's under it and opens its menu.
    fn menu_at(&mut self, ev: MouseEvent, _: Instant) {
        self.select_at(ev.column, ev.row);
        self.open_menu();
    }

    /// A click selects (a part of a post focuses it); a double click opens it.
    fn click_view(&mut self, ev: MouseEvent, now: Instant) {
        let Some(at) = self.select_at(ev.column, ev.row) else { return };
        if self.double_click(now, at) {
            self.on_view_key(KeyEvent::from(KeyCode::Enter));
        }
    }

    /// A click on a gallery file selects it; a double click views it.
    fn on_gallery_click(&mut self, ev: MouseEvent, now: Instant) {
        let Some(k) = self.select_at(ev.column, ev.row) else { return };
        if self.double_click(now, k) {
            self.view_from_gallery(k);
        }
    }

    /// Whether a click on `at` is the second of a double click: on the same row of the same
    /// frame as the one before (see `forget_frame`), and soon after it.
    pub(super) fn double_click(&mut self, now: Instant, at: usize) -> bool {
        if self.last_click.take().is_some_and(|(t, was)| was == at && now.duration_since(t) < Duration::from_millis(400)) {
            return true;
        }
        self.last_click = Some((now, at));
        false
    }

    /// What the last frame drew may have moved: no click lands until the next frame, and
    /// none before pairs with one after into a double click.
    pub(super) fn forget_frame(&mut self) {
        self.drawn.fresh = false;
        self.last_click = None;
    }

    /// What's on screen now, as a frame stamps it.
    pub(super) fn shown(&self) -> Shown {
        (self.screen(), self.tabs.len(), self.modal())
    }

    /// Whether a popup or input is taking the keys (the fuzzer asks).
    #[cfg(test)]
    pub(super) fn modal_open(&self) -> Option<()> {
        self.modal().map(|_| ())
    }

    /// What's capturing input, topmost first (the viewer can be open over the gallery, and
    /// image search over the viewer). It gets every key; the mouse goes as `mouse` says.
    pub(super) fn modal(&self) -> Option<Modal> {
        if let Some(p) = &self.popup {
            return Some(match p {
                Popup::Settings(_) => Modal::Settings,
                Popup::Menu(_) => Modal::Menu,
                Popup::Hints(_) => Modal::Hints,
                Popup::Confirm(_) => Modal::Confirm,
                Popup::Adding(_) => Modal::Adding,
                Popup::AddFilter(_) => Modal::AddFilter,
                Popup::ImageSearch(_) => Modal::ImageSearch,
                Popup::Help(_) => Modal::Help,
                Popup::Reply(_) => Modal::Reply,
            });
        }
        let open = [
            (self.tab.viewer().is_some(), Modal::Viewer),
            (matches!(self.tab.popup, Some(TabPopup::Preview(_))), Modal::Preview),
            (self.tab.gallery.is_some() && self.tab.view() == View::Thread, Modal::Gallery),
            (matches!(self.tab.popup, Some(TabPopup::Links(_))), Modal::Links),
        ];
        open.into_iter().find_map(|(on, modal)| on.then_some(modal)).or_else(|| self.typing.as_ref().map(|t| match t {
            Typing::Goto(_) => Modal::Goto,
            Typing::ArchiveQuery(_) => Modal::SearchInput,
            Typing::ThreadSearch => Modal::Searching,
            Typing::ListFilter => Modal::Filtering,
        }))
    }

    /// Select what was drawn at a screen position: a list row, a gallery file, or a post (and
    /// its part) in a thread. Which, if anything.
    fn select_at(&mut self, col: u16, row: u16) -> Option<usize> {
        let at = self.click_target(col, row);
        let thread = self.tab.view() == View::Thread;
        if let Some(g) = self.tab.gallery.as_mut().filter(|_| thread) {
            let k = at.filter(|&k| k < g.files.len())?;
            g.state.select(Some(k));
            return Some(k);
        }
        if thread {
            let (e, part) = self.thread_part_at(col, row)?;
            let t = self.tab.thread.as_mut()?;
            t.set_cursor(e);
            if t.focus != part {
                t.focus = part;
                t.layout = None;
            }
            return Some(e);
        }
        let target = at?;
        self.pick_row(self.tab.view(), target).then_some(target)
    }

    /// The list row or grid cell drawn at a screen position.
    fn click_target(&self, col: u16, row: u16) -> Option<usize> {
        self.drawn()?.body?.row_at(col, row)
    }

    /// The row of the popup's list drawn at a screen position.
    pub(super) fn popup_row(&self, col: u16, row: u16) -> Option<usize> {
        self.drawn()?.popup?.row_at(col, row)
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
        match self.modal() {
            Some(modal) => self.on_modal_key(modal, key),
            None => self.on_view_key(key),
        }
    }

    /// A key for what's on top.
    fn on_modal_key(&mut self, modal: Modal, key: KeyEvent) {
        match modal {
            Modal::Confirm => self.on_confirm_key(key),
            Modal::Adding => self.on_adding_key(key),
            Modal::Settings => self.on_settings_popup_key(key),
            Modal::AddFilter => self.on_add_filter_key(key),
            Modal::Menu => self.on_menu_key(key),
            Modal::Hints => self.on_hints_key(key),
            Modal::Help => match (&mut self.popup, key.code) {
                (Some(Popup::Help(s)), KeyCode::Char('j') | KeyCode::Down) => scroll_by(s, 1),
                (Some(Popup::Help(s)), KeyCode::Char('k') | KeyCode::Up) => scroll_by(s, -1),
                _ => self.popup = None,
            },
            Modal::ImageSearch => self.on_image_search_key(key.code),
            Modal::Reply => self.on_reply_key(key),
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
    }

    /// A key for the view, with nothing on top: vim's way, a count before it (`10j`) and
    /// `z` / `g` taking a second key (`zz`, `gg`).
    fn on_view_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let pending = std::mem::take(&mut self.pending);
        // A key that doesn't finish the prefix drops it and counts as itself (esc only drops it).
        if let Some(prefix) = pending.prefix
            && (key.code == KeyCode::Esc || self.finish_prefix(prefix, key.code, pending.count))
        {
            return;
        }
        let n = pending.count.unwrap_or(1);
        let action = self.keys.action(self.scope(), &key);
        if let (None, KeyCode::Char(c), false) = (action, key.code, ctrl || key.modifiers.contains(KeyModifiers::ALT)) {
            match c {
                // A count: digits, where they aren't keys of their own (home's favorites); a
                // 0 only after another.
                '0'..='9' if self.tab.view() != View::Sites && (c != '0' || pending.count.is_some()) => {
                    let count = pending.count.unwrap_or(0).saturating_mul(10).saturating_add(c as usize - '0' as usize);
                    self.pending = Pending { count: Some(count.min(99_999)), prefix: None };
                    return;
                }
                // `5g`, `5G`: the fifth post (or entry) at once.
                'g' | 'G' if pending.count.is_some() => return self.go_to_entry(n),
                'g' => {
                    self.pending = Pending { count: None, prefix: Some('g') };
                    return;
                }
                'z' if self.tab.view() == View::Thread => {
                    self.pending = Pending { count: None, prefix: Some('z') };
                    return;
                }
                _ => {}
            }
        }
        if self.tab.view() == View::Search && self.keys.keys(Action::NextMatch).contains(&crate::keys::Key::from_event(&key)) {
            if self.more_results() {
                self.load_search_page();
            }
            return;
        }
        if let Some(action) = action {
            // Moves repeat with a count; anything else (a toggle) once.
            let repeats = matches!(action, Action::NextMatch | Action::PrevMatch | Action::JumpBack | Action::JumpForward | Action::NextTab | Action::PrevTab | Action::NextPart | Action::PrevPart);
            for _ in 0..if repeats { n } else { 1 } {
                self.act(action);
            }
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
            KeyCode::Esc if self.tab.view() == View::Thread && self.tab.thread.as_ref().is_some_and(|t| !t.search.is_empty()) => {
                if let Some(t) = &mut self.tab.thread {
                    t.set_search(String::new());
                }
            }
            // Out of a conversation, to the whole thread.
            KeyCode::Esc if self.tab.view() == View::Thread && self.tab.thread.as_ref().is_some_and(|t| t.conversation.is_some()) => {
                if let Some(t) = &mut self.tab.thread {
                    t.leave_conversation();
                }
            }
            KeyCode::Esc if !self.filter(self.tab.view()).is_empty() => self.edit_filter(self.tab.view(), String::clear),
            KeyCode::Esc => self.back(),
            _ if self.tab.view() == View::Catalog && self.on_grid_key(key.code) => {}
            KeyCode::Char(c @ '1'..='9') if self.tab.view() == View::Sites => self.open_favorite(c as usize - '1' as usize),
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => self.back(),
            _ if self.tab.view() == View::Thread => self.on_thread_key(key.code, ctrl, n),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.enter(),
            code => {
                let view = self.tab.view();
                let step = match code {
                    KeyCode::Char('j') | KeyCode::Down => 1,
                    KeyCode::Char('k') | KeyCode::Up => -1,
                    KeyCode::Char('e') if ctrl => 1,
                    KeyCode::Char('y') if ctrl => -1,
                    KeyCode::Char('d') if ctrl => 10,
                    KeyCode::Char('u') if ctrl => -10,
                    KeyCode::Char('f') if ctrl => 20,
                    KeyCode::Char('b') if ctrl => -20,
                    KeyCode::PageDown => 10,
                    KeyCode::PageUp => -10,
                    KeyCode::Home => isize::MIN / 2,
                    KeyCode::Char('G') | KeyCode::End => isize::MAX / 2,
                    _ => return,
                };
                self.move_selection(view, step.saturating_mul(n as isize));
                if view == View::Search {
                    self.search_moved();
                }
            }
        }
    }

    pub fn scope(&self) -> Scope {
        match self.tab.view() {
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
            Action::Help => self.popup = Some(Popup::Help(ListState::default())),
            Action::Settings => self.tab.navigate(View::Settings),
            Action::Search if matches!(self.tab.view(), View::Settings | View::Search) => {}
            Action::Search if self.tab.view() == View::Thread => {
                if let Some(t) = &mut self.tab.thread {
                    t.set_search(String::new());
                    self.typing = Some(Typing::ThreadSearch);
                }
            }
            Action::Search => self.typing = Some(Typing::ListFilter),
            Action::Reload => self.refresh(),
            Action::JumpBack if self.tab.view() != View::Thread => self.travel(true),
            Action::JumpForward => self.travel(false),
            Action::Watched => self.tab.navigate(View::Watched),
            Action::Filter => self.open_add_filter(),
            Action::Conversation => self.toggle_conversation(),
            Action::Poster => self.toggle_poster(),
            Action::Media => self.cycle_media(),
            Action::Browser | Action::Copy if self.focused_file().is_some_and(|f| f.link().is_none()) => self.info(super::NEITHER),
            Action::Browser => match self.focused_url() {
                Some((what, url)) => self.open_link(what, &url),
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
            Action::Goto => self.typing = Some(Typing::Goto(String::new())),
            Action::Links => self.open_links(),
            Action::Hide => self.toggle_hidden(),
            Action::Mine => self.toggle_mine(),
            Action::Reply => self.open_reply(super::posting::Quoting::Number),
            Action::Quote => self.open_reply(super::posting::Quoting::Text),
            Action::Gallery => self.open_gallery(),
            Action::Export => self.ask_to_save(Saving::Page),
            Action::ArchiveSearch => self.start_archive_search(),
            Action::ImageSearch => self.open_image_search(),
            Action::NewTab => self.new_tab(),
            Action::Favorite => self.toggle_favorite(),
            Action::AddSite => self.popup = Some(Popup::Adding(super::Adding::Typing(String::new()))),
            Action::BoardImages => self.toggle_board_images(),
            Action::UpdateBoards if self.tab.view() == View::Boards => self.refresh_board_list(self.tab.site),
            Action::UpdateBoards => {}
            Action::SearchSaved => self.typing = Some(Typing::Goto("saved ".into())),
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
                self.tab.catalog_list = FilteredList::on(None, std::mem::take(&mut self.tab.catalog_list.filter));
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
                    self.open_key(&key);
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
        self.edit_filter(self.tab.view(), |f| if key.code == KeyCode::Esc { f.clear() } else { edit_text(f, key.code) });
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter) || self.tab.view() == View::Thread {
            self.typing = None;
        }
    }

    /// Moving in the catalog grid: j/k by rows, h/l by columns (h in the first column goes
    /// back, as in lists), as the grid was last drawn; before a grid's first frame, by one card.
    /// Returns whether the key was one of those.
    fn on_grid_key(&mut self, code: KeyCode) -> bool {
        let (cols, edges) = match self.drawn_cols() {
            Some(Some(cols)) => (cols as isize, true),
            None if self.layout() == CatalogLayout::Grid => (1, false),
            _ => return false,
        };
        let len = self.visible_catalog().len();
        let cur = self.selected_row(View::Catalog).unwrap_or(0) as isize;
        let delta = match code {
            KeyCode::Char('j') | KeyCode::Down => cols,
            KeyCode::Char('k') | KeyCode::Up => -cols,
            KeyCode::Char('l') | KeyCode::Right if !edges || (cur + 1) % cols != 0 => 1,
            KeyCode::Char('l') | KeyCode::Right => 0,
            KeyCode::Char('h') | KeyCode::Left if !edges || cur % cols != 0 => -1,
            _ => return false,
        };
        // Down from the last full row goes to the last thread; past either end, nowhere.
        let last_row = delta == cols && cur + cols >= len as isize && cur / cols < (len as isize - 1) / cols;
        if let Ok(to) = usize::try_from(if last_row { len as isize - 1 } else { cur + delta }) {
            self.pick_row(View::Catalog, to);
        }
        true
    }

    /// A thread's own key, `n` times over where it's a move.
    fn on_thread_key(&mut self, code: KeyCode, ctrl: bool, n: usize) {
        if code == KeyCode::Enter && self.take_saved_offer() {
            return;
        }
        let Some(t) = &mut self.tab.thread else { return };
        let half = (t.viewport / 2).max(1) as isize;
        let times = n as isize;
        match code {
            KeyCode::Char('j') | KeyCode::Down if !ctrl => (0..n).for_each(|_| t.step(true)),
            KeyCode::Char('k') | KeyCode::Up if !ctrl => (0..n).for_each(|_| t.step(false)),
            KeyCode::Char('J') => t.scroll_lines(times),
            KeyCode::Char('K') => t.scroll_lines(-times),
            KeyCode::Char('e') if ctrl => t.scroll_lines(times),
            KeyCode::Char('y') if ctrl => t.scroll_lines(-times),
            KeyCode::Char('d') if ctrl => t.scroll_lines(half.saturating_mul(times)),
            KeyCode::Char('u') if ctrl => t.scroll_lines(-half.saturating_mul(times)),
            KeyCode::PageDown | KeyCode::Char(' ') => t.scroll_lines((half * 2 - 1).saturating_mul(times)),
            KeyCode::Char('f') if ctrl => t.scroll_lines((half * 2 - 1).saturating_mul(times)),
            KeyCode::PageUp => t.scroll_lines(-(half * 2 - 1).saturating_mul(times)),
            KeyCode::Char('b') if ctrl => t.scroll_lines(-(half * 2 - 1).saturating_mul(times)),
            KeyCode::Home => t.select_entry(0),
            // The very end of the thread, so a refresh's new posts come into view.
            KeyCode::Char('G') | KeyCode::End => {
                t.select_entry(usize::MAX);
                t.scroll_to(Reveal::Bottom);
                t.reveal = Some(Reveal::Bottom);
            }
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

    /// `z` or `g` with its second key: `zz` / `zt` / `zb` put the selected post at the
    /// screen's middle, top or bottom; `gg` goes to the top (or, with a count, that entry).
    /// False if `code` isn't one of these.
    fn finish_prefix(&mut self, prefix: char, code: KeyCode, count: Option<usize>) -> bool {
        let at = match (prefix, code) {
            ('g', KeyCode::Char('g')) => {
                self.go_to_entry(count.unwrap_or(1));
                return true;
            }
            ('z', KeyCode::Char('z' | '.')) => Place::Middle,
            ('z', KeyCode::Char('t') | KeyCode::Enter) => Place::Top,
            ('z', KeyCode::Char('b' | '-')) => Place::Bottom,
            _ => return false,
        };
        if let Some(t) = &mut self.tab.thread {
            t.place(at);
        }
        true
    }

    /// The `n`th entry (from 1): a thread's post, or a list's row.
    fn go_to_entry(&mut self, n: usize) {
        let view = self.tab.view();
        match &mut self.tab.thread {
            Some(t) if view == View::Thread => {
                t.select_entry(n.saturating_sub(1));
                t.scroll_to(Reveal::Jump);
            }
            _ => {
                self.move_selection(view, isize::MIN / 2);
                self.move_selection(view, isize::try_from(n.saturating_sub(1)).unwrap_or(isize::MAX / 2));
            }
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
            Action::Spoiler => t.toggle_spoiler(),
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
            // Back to the post jumped from; else to the thread before.
            Action::JumpBack => {
                if !t.jump_back() {
                    self.travel(true);
                }
            }
            Action::Unread => match (0..t.posts.len()).find(|&i| t.is_new(i)) {
                Some(i) => t.jump(i),
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
            self.typing = None;
            return;
        };
        match key.code {
            KeyCode::Esc => {
                t.set_search(String::new());
                self.typing = None;
            }
            KeyCode::Enter => {
                self.typing = None;
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
        let posts: Vec<u64> = p.quotes.iter().copied().filter(|q| t.index.contains_key(q)).collect();
        let elsewhere: Vec<u64> = p.links.iter().filter_map(|l| l.post).filter(|n| !t.index.contains_key(n)).collect();
        if posts.is_empty() && elsewhere.is_empty() {
            self.info("Post quotes nothing");
            return;
        }
        self.tab.popup = Some(TabPopup::Preview(Preview { posts, elsewhere, scroll: ListState::default() }));
    }

    fn on_preview_key(&mut self, code: KeyCode) {
        let Some(TabPopup::Preview(p)) = &mut self.tab.popup else { return };
        let s = &mut p.scroll;
        match code {
            KeyCode::Char('j') | KeyCode::Down => scroll_by(s, 1),
            KeyCode::Char('k') | KeyCode::Up => scroll_by(s, -1),
            KeyCode::Char(' ') | KeyCode::PageDown => scroll_by(s, 10),
            KeyCode::PageUp => scroll_by(s, -10),
            KeyCode::Char('g') => scroll_by(s, isize::MIN),
            // Jump to the (first) quoted post.
            KeyCode::Enter => {
                let first = p.posts.first().copied();
                self.tab.popup = None;
                if let (Some(t), Some(no)) = (&mut self.tab.thread, first) {
                    t.jump_to(no);
                }
            }
            _ => self.tab.popup = None,
        }
    }

    fn on_viewer_key(&mut self, code: KeyCode) {
        use crate::images::Crop;
        let Some(TabPopup::Viewer(v)) = &mut self.tab.popup else { return };
        let n = v.files.len();
        let zoomed = !v.crop.is_fit();
        match code {
            // Zoom: + and - (= is + without shift), 0 fits it again. Zoomed in, the arrows
            // and h/j/k/l move around, page up / down go to the other files, and esc fits.
            KeyCode::Char('+' | '=') => v.crop = v.crop.zoomed(true),
            KeyCode::Char('-') => v.crop = v.crop.zoomed(false),
            KeyCode::Char('0') => v.crop = Crop::FIT,
            KeyCode::Esc if zoomed => v.crop = Crop::FIT,
            KeyCode::Char('h') | KeyCode::Left if zoomed => v.crop = v.crop.moved(-1, 0, v.shown.unwrap_or_else(|| v.crop.guess_shown())),
            KeyCode::Char('l') | KeyCode::Right if zoomed => v.crop = v.crop.moved(1, 0, v.shown.unwrap_or_else(|| v.crop.guess_shown())),
            KeyCode::Char('k') | KeyCode::Up if zoomed => v.crop = v.crop.moved(0, -1, v.shown.unwrap_or_else(|| v.crop.guess_shown())),
            KeyCode::Char('j') | KeyCode::Down if zoomed => v.crop = v.crop.moved(0, 1, v.shown.unwrap_or_else(|| v.crop.guess_shown())),
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
                    && t.current().is_none_or(|p| p.no != no)
                {
                    t.select_post(no);
                }
                self.tab.popup = None;
            }
            KeyCode::Char('h' | 'k') | KeyCode::Left | KeyCode::Up => v.index = (v.index + n - 1) % n,
            // Space pauses an animated GIF; otherwise it's the next file.
            KeyCode::Char(' ') if v.files.get(v.index).is_some_and(|f| f.image().is_some_and(|u| self.images.toggle_pause(u))) => {
                let paused = v.files.get(v.index).and_then(|f| f.image()).is_some_and(|u| self.images.is_paused(u));
                self.info(if paused { "Paused (space plays)" } else { "Playing" });
            }
            KeyCode::Char('l' | 'j' | ' ') | KeyCode::Right | KeyCode::Down => v.index = (v.index + 1) % n,
            KeyCode::Char('i') | KeyCode::Enter => {
                if let Some(f) = v.files.get(v.index).cloned() {
                    self.open_file(&f);
                }
            }
            _ => {}
        }
    }
}

/// Scroll a popup whose rows aren't selected by `by` rows: the next frame's `ui::window`
/// clamps it to what it shows and keeps that, so a key back always moves the view.
#[allow(clippy::disallowed_methods)]
fn scroll_by(s: &mut ListState, by: isize) {
    *s.offset_mut() = s.offset().saturating_add_signed(by);
}

/// The arrow key a wheel notch moves rows by.
fn arrow(ev: MouseEvent) -> KeyEvent {
    KeyEvent::from(if ev.kind == ScrollDown { KeyCode::Down } else { KeyCode::Up })
}
