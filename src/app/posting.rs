//! Posting: the reply box (`P`), 4chan's captcha answered in the terminal, and the post sent
//! through ck-web, the browser helper (`crate::web`). Drafts are kept per thread until sent.

use super::*;
use anyhow::Context;
use crate::captcha::{Solving, Task, Twister};
use crate::editor::Editor;
use crate::web::{self, Reply, Request};
use image::DynamicImage;
use ratatui_image::protocol::Protocol;

/// What to say when ck-web isn't there, and can't be downloaded for this system.
const NO_HELPER: &str = "Posting to 4chan needs ck-web, ck's browser helper, which isn't built for this system yet: \
                         see the manual (Posting) to build it, and set web_helper";

/// Where a post goes: a site's board, and a thread there (0: a new thread).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Where {
    pub site: String,
    pub board: String,
    pub thread: u64,
}

impl Where {
    pub fn new_thread(&self) -> bool {
        self.thread == 0
    }
}

/// The reply box's fields, in the order tab goes through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Options,
    Subject,
    Comment,
    File,
    Spoiler,
}

impl Field {
    const ALL: [Field; 6] = [Field::Name, Field::Options, Field::Subject, Field::Comment, Field::File, Field::Spoiler];
}

/// Where the box is in posting.
pub enum Stage {
    Writing,
    /// ck-web isn't here: asking to download it.
    Offer,
    /// Downloading it: the bytes so far, of how many.
    Installing { got: u64, size: Option<u64> },
    /// The captcha's been asked for.
    Asking,
    /// The site wants a person (Cloudflare's check, hCaptcha): the part of its page to
    /// click, and where that part is on the page.
    Person(Option<(DynamicImage, u32, u32)>),
    /// No captcha for a while (posting too often): until when, and the site's message.
    Waiting { until: Instant, message: String },
    Solving(Box<Solving>),
    /// The post's been sent.
    Sending,
}

/// A picture drawn in the box, kept encoded for where it was drawn: what it is (`Compose::art`
/// says), and the area.
pub struct Art {
    pub key: u64,
    pub area: Rect,
    pub proto: Protocol,
}

/// Where the browser view was drawn: the cells, and the page's pixels they show (from
/// `left`, `top`, `width` x `height`).
#[derive(Debug, Clone, Copy)]
pub struct ViewAt {
    pub area: Rect,
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

/// A post being written.
pub struct Compose {
    pub to: Where,
    pub name: Editor,
    pub options: Editor,
    pub subject: Editor,
    pub comment: Editor,
    /// A file to attach, by its path.
    pub file: Editor,
    pub spoiler: bool,
    pub field: Field,
    pub stage: Stage,
    /// What went wrong last, till the next try.
    pub problem: Option<String>,
    /// The picture drawn last, and where the browser view was drawn, for clicks.
    pub art: Option<Art>,
    pub view_at: Option<ViewAt>,
    /// Bumped with each new browser view, so it's drawn anew.
    pub frames: u64,
}

impl Compose {
    fn new(to: Where, name: &str, options: &str) -> Self {
        let line = |s: &str| {
            let mut e = Editor::default();
            e.set(s);
            e
        };
        Compose {
            to,
            name: line(name),
            options: line(options),
            subject: Editor::default(),
            comment: Editor::multiline(),
            file: Editor::default(),
            spoiler: false,
            field: Field::Comment,
            stage: Stage::Writing,
            problem: None,
            art: None,
            view_at: None,
            frames: 0,
        }
    }

    /// The fields this box has: no subject in a reply.
    pub fn fields(&self) -> impl Iterator<Item = Field> + '_ {
        Field::ALL.into_iter().filter(|&f| f != Field::Subject || self.to.new_thread())
    }

    pub fn editor(&mut self, field: Field) -> Option<&mut Editor> {
        match field {
            Field::Name => Some(&mut self.name),
            Field::Options => Some(&mut self.options),
            Field::Subject => Some(&mut self.subject),
            Field::Comment => Some(&mut self.comment),
            Field::File => Some(&mut self.file),
            Field::Spoiler => None,
        }
    }

    fn step_field(&mut self, forward: bool) {
        let fields: Vec<Field> = self.fields().collect();
        let i = fields.iter().position(|&f| f == self.field).unwrap_or(0);
        let n = fields.len().max(1);
        let to = if forward { (i + 1) % n } else { (i + n - 1) % n };
        self.field = fields.get(to).copied().unwrap_or(Field::Comment);
    }

    /// Nothing written: closing it keeps no draft.
    fn is_blank(&self) -> bool {
        self.comment.text().trim().is_empty() && self.file.is_empty() && self.subject.is_empty() && matches!(self.stage, Stage::Writing | Stage::Offer)
    }

    /// Quote post `no` (and its text, if given), on lines of their own at the cursor; a post
    /// already quoted isn't again.
    fn quote(&mut self, no: u64, text: Option<&str>) {
        let link = format!(">>{no}");
        let mut s = String::new();
        if !self.comment.text().contains(&link) {
            s = format!("{link}\n");
        }
        for line in text.unwrap_or_default().lines().filter(|l| !l.trim().is_empty()) {
            s.push('>');
            s.push_str(line);
            s.push('\n');
        }
        if s.is_empty() {
            return;
        }
        let at_line_start = self.comment.text().is_empty() || self.comment.text().ends_with('\n');
        if !at_line_start {
            self.comment.insert("\n");
        }
        self.comment.insert(&s);
    }

    /// The form's fields, with the captcha's answer.
    fn form(&self, challenge: &str, answer: &str) -> Vec<(String, String)> {
        let mut fields = vec![("name".into(), self.name.text().to_string()), ("email".into(), self.options.text().to_string())];
        if self.to.new_thread() {
            fields.push(("sub".into(), self.subject.text().to_string()));
        }
        fields.push(("com".into(), self.comment.text().to_string()));
        if self.spoiler && !self.file.is_empty() {
            fields.push(("spoiler".into(), "on".into()));
        }
        fields.push(("t-challenge".into(), challenge.to_string()));
        fields.push(("t-response".into(), answer.to_string()));
        fields
    }

    /// A key while writing: to the field, or moving between them.
    fn on_writing_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab => self.step_field(true),
            KeyCode::BackTab => self.step_field(false),
            KeyCode::Enter | KeyCode::Char(' ') if self.field == Field::Spoiler => self.spoiler = !self.spoiler,
            KeyCode::Down if self.field != Field::Comment => self.step_field(true),
            KeyCode::Up if self.field != Field::Comment => self.step_field(false),
            code => {
                let field = self.field;
                if self.editor(field).is_some_and(|e| e.on_key(key)) {
                    self.problem = None;
                } else if code == KeyCode::Enter {
                    self.step_field(true);
                }
            }
        }
    }

    /// Back to writing, saying what went wrong.
    fn problem(&mut self, what: impl Into<String>) {
        self.stage = Stage::Writing;
        self.problem = Some(what.into());
    }
}

/// What a key in the box leads to, past the box itself.
enum Then {
    Nothing,
    Close,
    Send,
    Ask,
    Post,
    Cancel,
    Install,
}

/// What a reply quotes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quoting {
    /// The selected post's number (not the opening post's).
    Number,
    /// Its number and its text.
    Text,
}

impl App {
    /// Whether ck can post where you are: 4chan, live (not a saved copy).
    pub(super) fn can_post(&self) -> bool {
        self.current_site().cfg.kind == SiteKind::Fourchan && self.tab.saved().is_none()
    }

    /// `P`: the reply box for the thread (quoting the selected post), or for a new thread in
    /// the catalog. A draft left there comes back.
    pub(super) fn open_reply(&mut self, quoting: Quoting) {
        let to = match self.tab.view() {
            View::Thread => self.tab.thread.as_ref().map(|t| Where { site: t.key().site.clone(), board: t.key().board.clone(), thread: t.key().no }),
            View::Catalog => self.tab.catalog.board().map(|b| Where { site: self.tab.catalog.site().to_string(), board: b.uri.clone(), thread: 0 }),
            _ => None,
        };
        let Some(to) = to else {
            self.info("Open a thread to reply, or a catalog to start a thread");
            return;
        };
        let fourchan = self.site_index(&to.site).and_then(|i| self.sites.get(i)).is_some_and(|s| s.cfg.kind == SiteKind::Fourchan);
        if !fourchan {
            let browser = self.keys.key(Action::Browser);
            self.info(format!("ck posts to 4chan only, so far; {browser} opens this in the browser"));
            return;
        }
        let mut c = self.drafts.remove(&to).unwrap_or_else(|| Compose::new(to, &self.poster.0, &self.poster.1));
        if let Some(t) = &self.tab.thread
            && let Some(p) = t.current().filter(|p| self.tab.view() == View::Thread && (p.no != t.key().no || quoting == Quoting::Text))
        {
            c.quote(p.no, (quoting == Quoting::Text).then(|| p.plain_text()));
        }
        c.field = Field::Comment;
        self.popup = Some(Popup::Reply(Box::new(c)));
    }

    /// Close the box; what's written is kept for next time. Asking for a captcha stops.
    fn close_reply(&mut self) {
        let Some(Popup::Reply(mut c)) = self.popup.take() else { return };
        if matches!(c.stage, Stage::Asking | Stage::Person(_)) {
            self.cancel_web();
            c.stage = Stage::Writing;
        }
        if matches!(c.stage, Stage::Offer) {
            c.stage = Stage::Writing;
        }
        c.art = None;
        if !c.is_blank() {
            self.drafts.insert(c.to.clone(), *c);
        }
    }

    fn cancel_web(&mut self) {
        if let Some(h) = &mut self.web {
            let _ = h.send(&Request::Cancel);
        }
        self.web_for = None;
    }

    /// The box open, if it is.
    fn reply_box(&mut self) -> Option<&mut Compose> {
        match &mut self.popup {
            Some(Popup::Reply(c)) => Some(c),
            _ => None,
        }
    }

    /// The post going to `to`: in the box, or a draft.
    fn compose_for(&mut self, to: &Where) -> Option<&mut Compose> {
        match &mut self.popup {
            Some(Popup::Reply(c)) if &c.to == to => Some(c),
            _ => self.drafts.get_mut(to),
        }
    }

    pub(super) fn on_reply_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(c) = self.reply_box() else { return };
        let then = match &mut c.stage {
            Stage::Writing => match key.code {
                KeyCode::Esc => Then::Close,
                KeyCode::Char('s') if ctrl => Then::Send,
                _ => {
                    c.on_writing_key(key);
                    Then::Nothing
                }
            },
            Stage::Offer if matches!(key.code, KeyCode::Char('y') | KeyCode::Enter) => Then::Install,
            Stage::Offer if matches!(key.code, KeyCode::Char('n') | KeyCode::Esc) => {
                c.stage = Stage::Writing;
                Then::Nothing
            }
            Stage::Installing { .. } if key.code == KeyCode::Esc => Then::Close,
            Stage::Asking | Stage::Person(_) if key.code == KeyCode::Esc => Then::Cancel,
            Stage::Waiting { .. } if key.code == KeyCode::Esc => {
                c.stage = Stage::Writing;
                Then::Nothing
            }
            Stage::Waiting { until, .. } if key.code == KeyCode::Enter && Instant::now() >= *until => Then::Send,
            Stage::Sending if key.code == KeyCode::Esc => Then::Close,
            Stage::Solving(_) if key.code == KeyCode::Esc => {
                c.stage = Stage::Writing;
                Then::Nothing
            }
            Stage::Solving(_) if ctrl && key.code == KeyCode::Char('r') => Then::Ask,
            Stage::Solving(s) if s.expired(Instant::now()) => {
                if key.code == KeyCode::Enter { Then::Ask } else { Then::Nothing }
            }
            Stage::Solving(s) => {
                if solve_key(s, key.code) { Then::Post } else { Then::Nothing }
            }
            Stage::Offer | Stage::Installing { .. } | Stage::Asking | Stage::Person(_) | Stage::Waiting { .. } | Stage::Sending => Then::Nothing,
        };
        match then {
            Then::Nothing => {}
            Then::Close => self.close_reply(),
            Then::Send => self.send_reply(),
            Then::Ask => self.ask_captcha(),
            Then::Post => self.post_reply(),
            Then::Install => self.install_helper(),
            Then::Cancel => {
                self.cancel_web();
                if let Some(c) = self.reply_box() {
                    c.stage = Stage::Writing;
                }
            }
        }
    }

    /// Text pasted into the box: into the field, or the captcha's answer.
    pub(super) fn paste_reply(&mut self, text: &str) -> bool {
        let Some(c) = self.reply_box() else { return false };
        match &mut c.stage {
            Stage::Writing => {
                let field = c.field;
                if let Some(e) = c.editor(field) {
                    e.insert(text);
                }
            }
            Stage::Solving(s) if matches!(s.challenge.task, Task::Text { .. }) => s.typed.push_str(text.trim()),
            _ => {}
        }
        true
    }

    /// Ctrl-s: check what's written, then ask for a captcha (or use the one solved, if it's
    /// still good).
    fn send_reply(&mut self) {
        let Some(c) = self.reply_box() else { return };
        if c.comment.text().trim().is_empty() && c.file.is_empty() {
            c.problem = Some("Write something or attach a file".into());
            return;
        }
        if !c.file.is_empty() && !crate::config::expand_home(c.file.text().trim()).is_file() {
            c.problem = Some(format!("No file at {}", c.file.text().trim()));
            c.field = Field::File;
            return;
        }
        let poster = (c.name.text().to_string(), c.options.text().to_string());
        self.poster = poster;
        if self.web.is_none() && web::find(self.web_helper.as_deref()).is_none() && web::install::bundle().is_some() {
            if let Some(c) = self.reply_box() {
                c.stage = Stage::Offer;
                c.problem = None;
            }
            return;
        }
        self.ask_captcha();
    }

    /// Download ck-web for the box's post, then go on to its captcha.
    fn install_helper(&mut self) {
        let Some(c) = self.reply_box() else { return };
        let to = c.to.clone();
        if crate::sandboxed() {
            c.problem("ck-web isn't downloaded here");
            return;
        }
        c.stage = Stage::Installing { got: 0, size: None };
        let later = self.later();
        std::thread::spawn(move || {
            let mut told = 0;
            let shown = to.clone();
            let progress = |got: u64, size: Option<u64>| {
                // A redraw a megabyte.
                if got >= told + (1 << 20) || size == Some(got) {
                    told = got;
                    let to = shown.clone();
                    later.run(move |app| {
                        if let Some(c) = app.compose_for(&to).filter(|c| matches!(c.stage, Stage::Installing { .. })) {
                            c.stage = Stage::Installing { got, size };
                        }
                    });
                }
            };
            let done = crate::guard::result(|| web::install::install(progress));
            later.run(move |app| app.helper_installed(&to, done));
        });
    }

    fn helper_installed(&mut self, to: &Where, done: anyhow::Result<std::path::PathBuf>) {
        let open = matches!(&self.popup, Some(Popup::Reply(c)) if &c.to == to);
        let Some(c) = self.compose_for(to) else { return };
        match done {
            Err(e) => c.problem(format!("Couldn't get ck-web: {e:#}")),
            Ok(_) if open => self.ask_captcha(),
            Ok(_) => c.stage = Stage::Writing,
        }
    }

    /// Ask ck-web for a captcha for the box's post.
    fn ask_captcha(&mut self) {
        let Some(to) = self.reply_box().map(|c| c.to.clone()) else { return };
        let asked = self.helper().and_then(|h| h.send(&Request::Captcha { board: to.board.clone(), thread: to.thread }));
        let Some(c) = self.reply_box() else { return };
        match asked {
            Ok(()) => {
                c.stage = Stage::Asking;
                c.problem = None;
                self.web_for = Some(to);
            }
            Err(e) => c.problem(e.to_string()),
        }
    }

    /// ck-web, started if it isn't running.
    fn helper(&mut self) -> anyhow::Result<&mut web::Helper> {
        if self.web.is_none() {
            if crate::sandboxed() {
                anyhow::bail!("ck-web isn't started here");
            }
            let path = web::find(self.web_helper.as_deref()).context(NO_HELPER)?;
            let later = self.later();
            self.web = Some(web::Helper::start(&path, move |reply| later.run(move |app| app.on_web(reply)))?);
        }
        self.web.as_mut().context(NO_HELPER)
    }

    /// What ck-web said (None: it's stopped).
    pub(super) fn on_web(&mut self, reply: Option<Reply>) {
        let Some(reply) = reply else {
            self.web = None;
            if let Some(to) = self.web_for.take()
                && let Some(c) = self.compose_for(&to)
                && matches!(c.stage, Stage::Asking | Stage::Person(_) | Stage::Sending)
            {
                c.problem("ck-web stopped");
            }
            return;
        };
        let Some(to) = self.web_for.clone() else { return };
        match reply {
            Reply::Ready => {}
            Reply::Captcha { twister } => {
                let Some(c) = self.compose_for(&to) else { return };
                match Twister::parse(&twister, Instant::now()) {
                    Twister::Refused(why) => c.problem(why),
                    Twister::Wait { until, message } => c.stage = Stage::Waiting { until, message },
                    Twister::Challenge(ch) => {
                        let none = matches!(ch.task, Task::None);
                        c.stage = Stage::Solving(Box::new(Solving::new(ch)));
                        c.art = None;
                        if none {
                            self.post_for(&to);
                        }
                    }
                }
            }
            Reply::View { png, left, top } => {
                let Some(c) = self.compose_for(&to) else { return };
                let img = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, png).ok().and_then(|b| image::load_from_memory(&b).ok());
                c.stage = Stage::Person(img.map(|i| (i, left, top)));
                c.frames += 1;
            }
            Reply::Posted { thread, no } => self.posted(&to, thread, no),
            Reply::Failed { error } => {
                self.web_for = None;
                if let Some(c) = self.compose_for(&to) {
                    c.problem(error);
                }
            }
        }
    }

    /// The captcha's answered: send the box's post.
    fn post_reply(&mut self) {
        if let Some(to) = self.reply_box().map(|c| c.to.clone()) {
            self.post_for(&to);
        }
    }

    fn post_for(&mut self, to: &Where) {
        let Some(c) = self.compose_for(to) else { return };
        let Stage::Solving(s) = &c.stage else { return };
        let Some(answer) = s.answer() else { return };
        let fields = c.form(&s.challenge.id, &answer);
        let file = Some(c.file.text().trim()).filter(|f| !f.is_empty()).map(|f| crate::config::expand_home(f).display().to_string());
        let request = Request::Post { board: to.board.clone(), thread: to.thread, fields, file };
        let sent = self.helper().and_then(|h| h.send(&request));
        let Some(c) = self.compose_for(to) else { return };
        match sent {
            Ok(()) => {
                c.stage = Stage::Sending;
                self.web_for = Some(to.clone());
            }
            Err(e) => c.problem(e.to_string()),
        }
    }

    /// The post's up: it's yours (the thread's watched for replies to it), and the thread's
    /// shown with it.
    fn posted(&mut self, to: &Where, thread: u64, no: u64) {
        self.web_for = None;
        let subject = self.compose_for(to).map(|c| c.subject.text().to_string()).unwrap_or_default();
        if matches!(&self.popup, Some(Popup::Reply(c)) if &c.to == to) {
            self.popup = None;
        }
        self.drafts.remove(to);
        let key = ThreadKey { site: to.site.clone(), board: to.board.clone(), no: thread };
        let open = self.tab.view() == View::Thread && self.tab.thread.as_ref().is_some_and(|t| t.key() == &key);
        if self.store.watched(&key).is_none() {
            let (subject, len, max_no) = match self.tab.thread.as_ref().filter(|_| open) {
                Some(t) => {
                    let posts = t.live_posts();
                    (thread_subject(&posts), t.known, max_no(&posts))
                }
                None => (subject, 0, 0),
            };
            self.store.watch(key.clone(), subject, len, max_no);
            self.keep_open_thread(&key);
        }
        if !self.store.watched(&key).is_some_and(|w| w.mine().contains(&no)) {
            self.rehide(|a| a.store.toggle_mine(&key, no));
        }
        self.save_now();
        if open {
            self.refresh();
        } else if to.new_thread() {
            self.open_key(&key);
        }
        self.info(format!("Posted No.{no}; it's marked as yours"));
    }

    /// A click on the reply box: on the browser view, to the page.
    pub(super) fn on_reply_click(&mut self, ev: MouseEvent, _: Instant) {
        if let Some((x, y)) = self.view_point(ev)
            && let Some(h) = &mut self.web
        {
            let _ = h.send(&Request::Click { x, y });
        }
    }

    /// The wheel over the browser view scrolls the page.
    pub(super) fn on_reply_wheel(&mut self, ev: MouseEvent, _: Instant) {
        let dy = if ev.kind == ratatui::crossterm::event::MouseEventKind::ScrollDown { 60 } else { -60 };
        if let Some((x, y)) = self.view_point(ev)
            && let Some(h) = &mut self.web
        {
            let _ = h.send(&Request::Scroll { x, y, dy });
        }
    }

    /// Where a mouse event is on the browser view, in the page's pixels (the middle of the
    /// cell).
    fn view_point(&mut self, ev: MouseEvent) -> Option<(u32, u32)> {
        let c = self.reply_box()?;
        let Stage::Person(_) = c.stage else { return None };
        let v = c.view_at?;
        if !v.area.contains(ratatui::layout::Position::new(ev.column, ev.row)) {
            return None;
        }
        let at = |cell: u16, start: u16, cells: u16, px: u32| (u32::from(cell - start) * 2 + 1) * px / (u32::from(cells.max(1)) * 2);
        Some((v.left + at(ev.column, v.area.x, v.area.width, v.width), v.top + at(ev.row, v.area.y, v.area.height, v.height)))
    }
}

/// A key while answering the captcha; whether it's answered now.
fn solve_key(s: &mut Solving, code: KeyCode) -> bool {
    match &s.challenge.task {
        Task::None => true,
        Task::Slider(_) => match code {
            KeyCode::Left | KeyCode::Char('h') => {
                s.slide(-1);
                false
            }
            KeyCode::Right | KeyCode::Char('l') => {
                s.slide(1);
                false
            }
            KeyCode::Enter | KeyCode::Char(' ') => s.next_step(),
            _ => false,
        },
        Task::Text { .. } => match code {
            KeyCode::Enter => s.answer().is_some(),
            KeyCode::Backspace => {
                s.typed.pop();
                false
            }
            KeyCode::Char(ch) => {
                s.typed.push(ch);
                false
            }
            _ => false,
        },
        Task::Grid { cells, .. } => {
            let n = cells.len();
            let cols = grid_cols(n);
            match code {
                KeyCode::Left | KeyCode::Char('h') => s.at = s.at.saturating_sub(1),
                KeyCode::Right | KeyCode::Char('l') => s.at = (s.at + 1).min(n.saturating_sub(1)),
                KeyCode::Up | KeyCode::Char('k') => s.at = s.at.saturating_sub(cols),
                KeyCode::Down | KeyCode::Char('j') => s.at = (s.at + cols).min(n.saturating_sub(1)),
                KeyCode::Char(' ') => s.toggle(s.at),
                KeyCode::Char(d @ '1'..='9') => s.toggle(d as usize - '1' as usize),
                KeyCode::Enter => return s.answer().is_some(),
                _ => {}
            }
            false
        }
    }
}

/// How many columns a captcha grid of `n` is drawn in.
pub fn grid_cols(n: usize) -> usize {
    match n {
        0..=3 => n.max(1),
        4 => 2,
        5..=9 => 3,
        _ => 4,
    }
}
