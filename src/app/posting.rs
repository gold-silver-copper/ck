//! Posting: the reply box (`P`), the site's captcha answered in the terminal, and the post
//! sent through ck-web, the browser helper (`crate::web`), the way the site's engine posts
//! (`crate::post`). Drafts are kept per thread until sent.

use super::*;
use anyhow::Context;
use crate::captcha::{Challenge, Solving, Task};
use crate::editor::Editor;
use crate::pty_editor::PtyEditor;
pub use crate::post::Where;
use crate::post::{Ask, LoginField, Draft, Login, Posted, Poster, Session, Web};
use std::sync::mpsc::{Sender, channel};
use crate::web::{self, Reply, Request};
use image::DynamicImage;
use ratatui_image::protocol::Protocol;
use std::sync::Arc;

/// What to say when ck-web isn't there, and can't be downloaded for this system.
const NO_HELPER: &str = "Posting needs ck-web, ck's browser helper, which isn't built for this system yet: \
                         see the manual (Posting) to build it, and set web_helper";

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

/// What tab completes a file's path to, as a shell would: the path with as much of the
/// names in its folder that start as it ends as they share (a folder's with a `/`), and
/// those names when there are several. None: no name starts so.
#[allow(clippy::disallowed_methods)] // your own folders, not ck's (folders listed too)
pub fn complete_path(typed: &str) -> Option<(String, Vec<String>)> {
    let (dir, start) = typed.rfind('/').map_or(("", typed), |i| typed.split_at(i + 1));
    let folder = if dir.is_empty() { std::path::PathBuf::from(".") } else { crate::config::expand_home(dir) };
    // Hidden names only when asked for.
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .ok()?
        .filter_map(|e| {
            let e = e.ok()?;
            let name = e.file_name().into_string().ok()?;
            let slash = if e.path().is_dir() { "/" } else { "" };
            (name.starts_with(start) && (start.starts_with('.') || !name.starts_with('.'))).then(|| format!("{name}{slash}"))
        })
        .collect();
    names.sort();
    let first = names.first()?;
    let common: String = names.iter().skip(1).fold(first.clone(), |acc, n| acc.chars().zip(n.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a).collect());
    let choices = if names.len() > 1 { names } else { Vec::new() };
    Some((format!("{dir}{common}"), choices))
}

/// Where the box is in posting.
pub enum Stage {
    Writing,
    /// ck-web isn't here: asking to download it.
    Offer,
    /// Downloading it: the bytes so far, of how many.
    Installing { got: u64, size: Option<u64> },
    /// The post is on its way: ck is talking to the site.
    Working,
    /// The site wants a person (Cloudflare's check, hCaptcha): the part of its page to
    /// click, and where that part is on the page.
    Person(Option<(DynamicImage, u32, u32)>),
    /// No captcha for a while (posting too often): until when, and the site's message.
    Waiting { until: Instant, message: String },
    Solving(Box<Solving>),
    /// The site's login (ctrl-p): asking what to log in with, or whether to log out.
    Login(Box<LoginForm>),
}

/// Logging in to a site, or out: what it asks for, typed (shown as dots), or, when ck-web's
/// logged in already, whether to log out.
pub struct LoginForm {
    pub logged_in: bool,
    pub fields: Vec<(LoginField, Editor)>,
    pub at: usize,
}

impl LoginForm {
    fn new(login: &dyn Login, logged_in: bool) -> Self {
        LoginForm { logged_in, fields: login.asks().iter().map(|&asked| (asked, Editor::default())).collect(), at: 0 }
    }

    /// A key while typing: whether it's done (enter with everything needed filled in).
    fn on_key(&mut self, key: KeyEvent) -> bool {
        let n = self.fields.len().max(1);
        match key.code {
            KeyCode::Enter if self.fields.iter().all(|(a, e)| a.optional || !e.text().trim().is_empty()) => return true,
            KeyCode::Enter | KeyCode::Tab | KeyCode::Down => self.at = (self.at + 1) % n,
            KeyCode::BackTab | KeyCode::Up => self.at = (self.at + n - 1) % n,
            _ => {
                if let Some((_, e)) = self.fields.get_mut(self.at) {
                    e.on_key(key);
                }
            }
        }
        false
    }
}

/// What's asked of a site's login.
enum LoginJob {
    Check,
    LogIn(Vec<String>),
    LogOut,
}

/// How it went.
pub enum LoginDone {
    Checked(bool),
    LoggedIn,
    LoggedOut,
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
    /// What went right (logged in), till the next key.
    pub note: Option<String>,
    /// What ck-web is doing for it, while it's working.
    pub doing: String,
    /// What the site can be logged in to with, if anything.
    pub login: Option<Arc<dyn Login>>,
    /// ck-web's being offered (or downloaded) for the login, not the post.
    for_login: bool,
    /// The picture drawn last, and where the browser view was drawn, for clicks.
    pub art: Option<Art>,
    /// The picture beside it (a slider's shape to find).
    pub side_art: Option<Art>,
    pub view_at: Option<ViewAt>,
    /// Bumped with each new browser view, so it's drawn anew.
    pub frames: u64,
    /// The keys' pointer on the browser view, in the page's pixels.
    pub pointer: Option<(u32, u32)>,
    /// Bumped by each try at posting and each stop, so a stopped try's end is dropped.
    attempt: u64,
    /// Where the answer to what's asked (a captcha, a wait) goes, back to the post on its
    /// way; dropped, it stops the post.
    answer: Option<Sender<Option<String>>>,
    /// The site's longest comment, where ck knows it.
    pub limit: Option<usize>,
    /// The names in the file's folder that tab could complete it to, listed after a tab
    /// that couldn't decide between them; the next tab moves on.
    pub file_choices: Vec<String>,
    /// The user's editor, writing the comment (ctrl-g), till it quits.
    pub pty: Option<PtyEditor>,
    /// Bumped for each clipboard read; only the pending read may fill the field.
    paste: u64,
    clipboard: Option<u64>,
}

impl Compose {
    fn new(to: Where, name: &str, options: &str, poster: &dyn Poster) -> Self {
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
            note: None,
            doing: String::new(),
            login: poster.login(),
            for_login: false,
            art: None,
            side_art: None,
            view_at: None,
            frames: 0,
            pointer: None,
            attempt: 0,
            answer: None,
            limit: poster.comment_limit(),
            file_choices: Vec::new(),
            pty: None,
            paste: 0,
            clipboard: None,
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

    /// The post as written, with the password to delete it by.
    fn draft(&self, password: &str) -> Draft {
        let file = Some(self.file.text().trim()).filter(|f| !f.is_empty()).map(crate::config::expand_home);
        Draft {
            name: self.name.text().to_string(),
            email: self.options.text().to_string(),
            subject: if self.to.new_thread() { self.subject.text().to_string() } else { String::new() },
            comment: self.comment.text().to_string(),
            spoiler: self.spoiler && file.is_some(),
            file,
            password: password.to_string(),
        }
    }

    /// Tab in the file's field: complete the path as a shell does. Whether it did anything
    /// (else tab moves on, as in the other fields).
    fn complete_file(&mut self) -> bool {
        let typed = self.file.text().to_string();
        let Some((done, choices)) = Some(typed.as_str()).filter(|t| !t.trim().is_empty()).and_then(complete_path) else { return false };
        if done != typed {
            self.file.set(&done);
            self.file_choices = choices;
            return true;
        }
        // Nothing more in common: the choices, once.
        if !choices.is_empty() && self.file_choices.is_empty() {
            self.file_choices = choices;
            return true;
        }
        false
    }

    /// A key while writing: to the field, or moving between them.
    fn on_writing_key(&mut self, key: KeyEvent) {
        self.note = None;
        if key.code != KeyCode::Tab {
            self.file_choices.clear();
        }
        match key.code {
            KeyCode::Tab if self.field == Field::File && self.complete_file() => {}
            KeyCode::Tab => {
                self.file_choices.clear();
                self.step_field(true);
            }
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

    /// Move the browser view's pointer: the arrows (or h/j/k/l) by about a cell as drawn,
    /// with shift by five.
    fn move_pointer(&mut self, key: KeyEvent) {
        let (Some((x, y)), Some(Stage::Person(Some((img, left, top))))) = (self.pointer, Some(&self.stage)) else { return };
        // A cell's worth of the page, as last drawn (before that, a guess).
        let (cw, ch) = self.view_at.map_or((8, 16), |v| (v.width / u32::from(v.area.width.max(1)), v.height / u32::from(v.area.height.max(1))));
        let far = key.modifiers.contains(KeyModifiers::SHIFT) || matches!(key.code, KeyCode::Char('H' | 'J' | 'K' | 'L'));
        let n = if far { 5 } else { 1 };
        let (dx, dy): (i64, i64) = match key.code {
            KeyCode::Left | KeyCode::Char('h' | 'H') => (-1, 0),
            KeyCode::Right | KeyCode::Char('l' | 'L') => (1, 0),
            KeyCode::Up | KeyCode::Char('k' | 'K') => (0, -1),
            KeyCode::Down | KeyCode::Char('j' | 'J') => (0, 1),
            _ => return,
        };
        let step = |at: u32, d: i64, cell: u32, start: u32, len: u32| {
            let to = i64::from(at) + d * n * i64::from(cell.max(1));
            u32::try_from(to.clamp(i64::from(start), i64::from(start + len.saturating_sub(1)))).unwrap_or(at)
        };
        self.pointer = Some((step(x, dx, cw, *left, img.width()), step(y, dy, ch, *top, img.height())));
    }

    /// Back to writing, saying what went wrong.
    fn problem(&mut self, what: impl Into<String>) {
        self.stage = Stage::Writing;
        self.answer = None;
        self.problem = Some(what.into());
    }

    /// Give what's asked its answer (None: another captcha), and wait on the site again.
    fn reply(&mut self, answer: Option<String>) {
        if let Some(tx) = self.answer.take() {
            let _ = tx.send(answer);
        }
        self.stage = Stage::Working;
    }

    /// Whether a post is on its way (and the box isn't for writing).
    fn busy(&self) -> bool {
        matches!(self.stage, Stage::Working | Stage::Person(_) | Stage::Waiting { .. } | Stage::Solving(_))
    }
}

/// A post just sent, until the thread shows it: 4chan's API has it a few seconds late, so
/// its thread is refreshed as often as the API allows (`MIN_REFETCH`), and a new thread is
/// opened only once it should be there.
pub struct Awaiting {
    pub key: ThreadKey,
    pub no: u64,
    /// When to stop waiting.
    pub until: Instant,
    /// A new thread: when to open it (again, after a 404).
    pub open_at: Option<Instant>,
}

/// How long a post is waited for.
const AWAIT: Duration = Duration::from_secs(120);
/// How long after posting a new thread to open it.
const NEW_THREAD_AFTER: Duration = Duration::from_secs(5);

/// What a key in the box leads to, past the box itself.
enum Then {
    Nothing,
    Close,
    Send,
    /// Ctrl-p: the site's login.
    OpenLogin,
    /// Ctrl-g: write the comment in $EDITOR.
    Edit,
    PasteFile,
    LogIn,
    LogOut,
    /// Stop the post on its way.
    Stop,
    /// Answer what it asked (None: another captcha).
    Reply(Option<String>),
    Install,
    /// Click the browser view where the pointer is.
    Click,
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
    /// Whether ck can post where you are: a site whose engine it posts to, live (not a
    /// saved copy).
    pub(crate) fn can_post(&self) -> bool {
        crate::post::poster(&self.current_site().cfg).is_some() && self.tab.saved().is_none()
    }

    /// Where the reply box would post from here: the thread, or a new thread on the
    /// catalog's board (an overboard has none).
    fn reply_to(&self) -> Option<Where> {
        match self.tab.view() {
            View::Thread => self.tab.thread.as_ref().map(|t| Where { site: t.key().site.clone(), board: t.key().board.clone(), thread: t.key().no }),
            View::Catalog => self.tab.catalog.board().filter(|_| !self.tab.catalog.mixes_boards()).map(|b| Where { site: self.tab.catalog.site().to_string(), board: b.uri.clone(), thread: 0 }),
            _ => None,
        }
    }

    /// A draft left here (esc in the reply box) waits for the reply box.
    pub(crate) fn draft_here(&self) -> bool {
        self.reply_to().is_some_and(|to| self.drafts.contains_key(&to))
    }

    /// `P`: the reply box for the thread (quoting the selected post), or for a new thread in
    /// the catalog. A draft left there comes back.
    pub(super) fn open_reply(&mut self, quoting: Quoting) {
        let Some(to) = self.reply_to() else {
            self.note(if self.tab.view() == View::Catalog && self.tab.catalog.mixes_boards() {
                "This has threads from many boards: start a thread from its board's catalog"
            } else {
                "Open a thread to reply, or a catalog to start a thread"
            });
            return;
        };
        let Some(poster) = self.poster_for(&to) else {
            let browser = self.keys.key(Action::Browser);
            self.note(format!("ck can't post on this site (an archive takes no posts); {browser} opens this in the browser"));
            return;
        };
        let mut c = self.drafts.remove(&to).unwrap_or_else(|| {
            let (name, options) = self.posters.get(&to.site).cloned().unwrap_or_default();
            Compose::new(to, &name, &options, poster.as_ref())
        });
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
        if c.busy() {
            self.cancel_web();
            c.attempt += 1;
            c.answer = None;
            c.stage = Stage::Writing;
        }
        // What was typed for a login goes.
        if matches!(c.stage, Stage::Offer | Stage::Login(_)) {
            c.stage = Stage::Writing;
        }
        c.art = None;
        c.side_art = None;
        if !c.is_blank() {
            self.drafts.insert(c.to.clone(), *c);
        }
    }

    fn cancel_web(&mut self) {
        if let Some(h) = &self.web {
            let _ = h.send(&Request::Cancel);
        }
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

    /// Ctrl-g: the comment written in the user's editor, in the box. Each time it draws, the
    /// loop wakes to show it; when it quits, `editor_quit` takes what it saved.
    fn open_editor(&mut self) {
        let (drew, quit) = (self.later(), self.later());
        let Some(c) = self.reply_box() else { return };
        let to = c.to.clone();
        let started = PtyEditor::start(
            c.comment.text(),
            move || {
                drew.run(|_| {});
            },
            move |ok| {
                quit.run(move |app| app.editor_quit(&to, ok));
            },
        );
        match started {
            Ok(pty) => {
                c.pty = Some(pty);
                c.field = Field::Comment;
                c.problem = None;
            }
            Err(e) => c.problem = Some(format!("Couldn't start the editor: {e:#}")),
        }
    }

    /// The editor quit: what it saved is the comment.
    fn editor_quit(&mut self, to: &Where, ok: bool) {
        let Some(c) = self.compose_for(to) else { return };
        let Some(pty) = c.pty.take() else { return };
        match pty.saved(ok) {
            Ok(Some(text)) => c.comment.set(&text),
            Ok(None) => c.note = Some("The editor quit with an error: the comment is as it was".into()),
            Err(e) => c.problem = Some(format!("Couldn't read what the editor saved: {e:#}")),
        }
    }

    pub(super) fn on_reply_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(c) = self.reply_box() else { return };
        // Every key is the editor's while it's open: it's left as it's left in a terminal.
        if let Some(pty) = &mut c.pty {
            pty.key(key);
            return;
        }
        let busy = c.busy();
        let then = match &mut c.stage {
            Stage::Writing => match key.code {
                KeyCode::Esc => Then::Close,
                KeyCode::Char('s') if ctrl => Then::Send,
                KeyCode::Char('p') if ctrl => Then::OpenLogin,
                KeyCode::Char('g') if ctrl => Then::Edit,
                KeyCode::Char('v') if ctrl && c.field == Field::File => Then::PasteFile,
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
            Stage::Login(_) if key.code == KeyCode::Esc => {
                c.stage = Stage::Writing;
                c.problem = None;
                Then::Nothing
            }
            Stage::Login(form) if form.logged_in => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => Then::LogOut,
                KeyCode::Char('n') => {
                    c.stage = Stage::Writing;
                    Then::Nothing
                }
                _ => Then::Nothing,
            },
            Stage::Login(form) => {
                if form.on_key(key) { Then::LogIn } else { Then::Nothing }
            }
            Stage::Installing { .. } if key.code == KeyCode::Esc => Then::Close,
            _ if key.code == KeyCode::Esc && busy => Then::Stop,
            Stage::Person(_) if matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) => Then::Click,
            Stage::Person(_) => {
                c.move_pointer(key);
                Then::Nothing
            }
            Stage::Waiting { until, .. } if key.code == KeyCode::Enter && Instant::now() >= *until => Then::Reply(Some(String::new())),
            // ctrl-r, or enter on one that's expired: another captcha.
            Stage::Solving(s) if (ctrl && key.code == KeyCode::Char('r')) || (key.code == KeyCode::Enter && s.expired(Instant::now())) => Then::Reply(None),
            Stage::Solving(s) => match solve_key(s, key.code).then(|| s.answer()).flatten() {
                Some(answer) => Then::Reply(Some(answer)),
                None => Then::Nothing,
            },
            Stage::Offer | Stage::Installing { .. } | Stage::Working | Stage::Waiting { .. } => Then::Nothing,
        };
        match then {
            Then::Nothing => {}
            Then::Close => self.close_reply(),
            Then::Send => self.send_reply(),
            Then::OpenLogin => self.open_login(),
            Then::Edit => self.open_editor(),
            Then::PasteFile => self.paste_file(),
            Then::LogIn => {
                // What was typed goes to the site, and isn't kept.
                let Some(c) = self.reply_box() else { return };
                let Stage::Login(form) = std::mem::replace(&mut c.stage, Stage::Writing) else { return };
                let answers = form.fields.into_iter().map(|(_, e)| e.text().to_string()).collect();
                self.run_login(LoginJob::LogIn(answers));
            }
            Then::LogOut => self.run_login(LoginJob::LogOut),
            Then::Install => self.install_helper(),
            Then::Click => {
                if let Some((x, y)) = self.reply_box().and_then(|c| c.pointer) {
                    self.click_page(x, y);
                }
            }
            Then::Reply(answer) => {
                if let Some(c) = self.reply_box() {
                    c.reply(answer);
                }
            }
            Then::Stop => {
                self.cancel_web();
                if let Some(c) = self.reply_box() {
                    c.attempt += 1;
                    c.answer = None;
                    c.stage = Stage::Writing;
                }
            }
        }
    }

    /// Ctrl-v in File: the clipboard's file or image, read off the UI thread.
    fn paste_file(&mut self) {
        if crate::sandboxed() {
            return;
        }
        let later = self.later();
        let Some(c) = self.reply_box() else { return };
        if c.clipboard.is_some() {
            return;
        }
        c.paste += 1;
        let paste = c.paste;
        c.clipboard = Some(paste);
        let (to, before) = (c.to.clone(), c.file.text().to_string());
        c.note = Some("Reading clipboard…".into());
        std::thread::spawn(move || {
            let result = crate::clipboard::read_file();
            later.run(move |app| {
                let Some(c) = app.compose_for(&to) else { return };
                if c.clipboard != Some(paste) {
                    return;
                }
                c.clipboard = None;
                c.note = None;
                if !matches!(c.stage, Stage::Writing) || c.file.text() != before {
                    return;
                }
                match result {
                    Ok(path) => {
                        c.file.set(&path);
                        c.file_choices.clear();
                        c.problem = None;
                    }
                    Err(e) => c.problem = Some(format!("Couldn't paste a file: {e:#}")),
                }
            });
        });
    }

    /// Text pasted into the box: into the field, or the captcha's answer.
    pub(super) fn paste_reply(&mut self, text: &str) -> bool {
        let Some(c) = self.reply_box() else { return false };
        if let Some(pty) = &mut c.pty {
            pty.paste(text);
            return true;
        }
        match &mut c.stage {
            Stage::Writing => {
                let field = c.field;
                if field == Field::File {
                    c.file.set(&crate::clipboard::file_path(text));
                    c.file_choices.clear();
                } else if let Some(e) = c.editor(field) {
                    e.insert(text);
                }
            }
            Stage::Solving(s) if matches!(s.challenge.task, Task::Text { .. }) => s.typed.push_str(text.trim()),
            Stage::Login(form) => {
                if let Some((_, e)) = form.fields.get_mut(form.at) {
                    e.insert(text.trim());
                }
            }
            _ => {}
        }
        true
    }

    /// Ctrl-s: check what's written, then send it on its way.
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
        let site = c.to.site.clone();
        self.posters.insert(site, poster);
        if !self.offer_helper(false) {
            self.start_post();
        }
    }

    /// When ck-web isn't here but can be downloaded: offer it, for the post or the login.
    fn offer_helper(&mut self, login: bool) -> bool {
        if self.web.is_some() || web::find(self.web_helper.as_deref()).is_some() || web::install::bundle().is_none() {
            return false;
        }
        if let Some(c) = self.reply_box() {
            c.stage = Stage::Offer;
            c.for_login = login;
            c.problem = None;
        }
        true
    }

    /// Ctrl-p: whether ck-web is logged in to the site, to log in or out.
    fn open_login(&mut self) {
        let Some(c) = self.reply_box() else { return };
        if c.login.is_none() {
            c.problem = Some(format!("{} has nothing ck can log in to", c.to.site));
            return;
        }
        if !self.offer_helper(true) {
            self.run_login(LoginJob::Check);
        }
    }

    /// Check, log in or log out of the box's site, through ck-web on a thread of its own (a
    /// site's check before it is shown in the box, as for a post).
    fn run_login(&mut self, job: LoginJob) {
        let Some((to, login)) = self.reply_box().and_then(|c| Some((c.to.clone(), c.login.clone()?))) else { return };
        let helper = match self.helper().cloned() {
            Ok(h) => h,
            Err(e) => {
                if let Some(c) = self.reply_box() {
                    c.problem(e.to_string());
                }
                return;
            }
        };
        let name = login.name();
        let doing = match job {
            LoginJob::Check => format!("Checking your {name}"),
            LoginJob::LogIn(_) => format!("Logging in with your {name}"),
            LoginJob::LogOut => format!("Logging out of your {name}"),
        };
        let Some(attempt) = self.start_attempt(&to, Stage::Working) else { return };
        if let Some(c) = self.reply_box() {
            c.doing = doing;
        }
        let (later, logging_in) = (self.later(), matches!(job, LoginJob::LogIn(_)));
        std::thread::spawn(move || {
            let done = crate::guard::result(|| {
                helper.open(&login.page())?;
                Ok(match job {
                    LoginJob::Check => LoginDone::Checked(login.logged_in(&helper)?),
                    LoginJob::LogIn(answers) => login.log_in(&helper, &answers).map(|()| LoginDone::LoggedIn)?,
                    LoginJob::LogOut => login.log_out(&helper).map(|()| LoginDone::LoggedOut)?,
                })
            });
            later.run(move |app| app.login_done(&to, attempt, logging_in, done));
        });
    }

    /// How the login's check, login or logout went: the form, or back to writing saying so. A
    /// login the site refused asks again.
    pub(super) fn login_done(&mut self, to: &Where, attempt: u64, logging_in: bool, done: anyhow::Result<LoginDone>) {
        let Some(c) = self.compose_for(to).filter(|c| c.attempt == attempt && c.busy()) else { return };
        let Some(login) = c.login.clone() else { return };
        let name = login.name();
        match done {
            Ok(LoginDone::Checked(logged_in)) => c.stage = Stage::Login(Box::new(LoginForm::new(login.as_ref(), logged_in))),
            Ok(LoginDone::LoggedIn) => {
                c.stage = Stage::Writing;
                let skips = if login.skips_captcha() { format!(": posts on {} skip the captcha", c.to.site) } else { String::new() };
                c.note = Some(format!("Logged in with your {name}{skips}"));
            }
            Ok(LoginDone::LoggedOut) => {
                c.stage = Stage::Writing;
                c.note = Some(format!("Logged out of your {name}"));
            }
            Err(e) if logging_in => {
                c.stage = Stage::Login(Box::new(LoginForm::new(login.as_ref(), false)));
                c.problem = Some(format!("{e:#}"));
            }
            Err(e) => c.problem(format!("{e:#}")),
        }
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
        let for_login = c.for_login;
        match done {
            Err(e) => c.problem(format!("Couldn't get ck-web: {e:#}")),
            Ok(_) if open && for_login => self.run_login(LoginJob::Check),
            Ok(_) if open => self.start_post(),
            Ok(_) => c.stage = Stage::Writing,
        }
    }

    /// How the site of `to` posts.
    fn poster_for(&self, to: &Where) -> Option<Arc<dyn Poster>> {
        self.site_index(&to.site).and_then(|i| self.sites.get(i)).and_then(|s| crate::post::poster(&s.cfg))
    }

    /// Send the box's post on its way, on a thread of its own: the site's engine posts it
    /// through ck-web, asking the box what only the person can answer.
    fn start_post(&mut self) {
        let Some(to) = self.reply_box().map(|c| c.to.clone()) else { return };
        // The box only opens where the site's engine posts.
        let Some(poster) = self.poster_for(&to) else { return };
        let helper = match self.helper().cloned() {
            Ok(h) => h,
            Err(e) => {
                if let Some(c) = self.reply_box() {
                    c.problem(e.to_string());
                }
                return;
            }
        };
        let password = self.post_password.clone();
        let Some(draft) = self.compose_for(&to).map(|c| c.draft(&password)) else { return };
        let Some(attempt) = self.start_attempt(&to, Stage::Working) else { return };
        if let Some(c) = self.compose_for(&to) {
            c.doing = format!("Posting to {}", c.to.site);
        }
        let (later, work) = (self.later(), self.work);
        std::thread::spawn(move || {
            let ask = BoxAsk { later: later.clone(), to: to.clone(), attempt };
            let sent = crate::guard::result(|| {
                helper.open(&poster.page())?;
                poster.post(&Session { web: &helper, ask: &ask, work }, &to, &draft)
            });
            later.run(move |app| app.sent(&to, attempt, sent));
        });
    }

    /// What the post on its way asks: a captcha (`challenge`), or a wait (`until`, with the
    /// site's message); the answer goes back on `answer`.
    pub(super) fn asked(&mut self, to: &Where, attempt: u64, asked: Asked, answer: Sender<Option<String>>) {
        let Some(c) = self.compose_for(to).filter(|c| c.attempt == attempt && c.busy()) else { return };
        c.answer = Some(answer);
        match asked {
            Asked::Wait { until, message } => c.stage = Stage::Waiting { until, message },
            Asked::Solve(mut challenge) => {
                c.art = None;
                c.side_art = None;
                let pictures = std::mem::take(&mut challenge.pictures);
                c.stage = Stage::Solving(Box::new(Solving::new(challenge)));
                for (key, img) in pictures {
                    self.images.insert_decoded(&key, enlarged(img));
                }
            }
        }
    }

    /// How the post on its way ended: up (yours, even if the box was closed meanwhile), or
    /// not, and the box says why (unless it was stopped).
    pub(super) fn sent(&mut self, to: &Where, attempt: u64, sent: anyhow::Result<Posted>) {
        match sent {
            Ok(Posted { thread, no }) => self.posted(to, thread, no),
            Err(e) => {
                if let Some(c) = self.compose_for(to).filter(|c| c.attempt == attempt && c.busy()) {
                    c.problem(format!("{e:#}"));
                }
            }
        }
    }

    /// Put the box for `to` at `stage` for a new try: its number, for the answers to find.
    pub(super) fn start_attempt(&mut self, to: &Where, stage: Stage) -> Option<u64> {
        let c = self.compose_for(to)?;
        c.attempt += 1;
        c.stage = stage;
        c.problem = None;
        c.note = None;
        Some(c.attempt)
    }

    /// ck-web, started if it isn't running.
    fn helper(&mut self) -> anyhow::Result<&web::Helper> {
        if self.web.is_none() {
            if crate::sandboxed() {
                anyhow::bail!("ck-web isn't started here");
            }
            let path = web::find(self.web_helper.as_deref()).context(NO_HELPER)?;
            let (views, stops) = (self.later(), self.later());
            let on_view = move |view| {
                views.run(move |app| app.on_view(view));
            };
            let on_stop = move |said: Option<String>| {
                stops.run(move |app| app.on_helper_stop(said));
            };
            self.web = Some(web::Helper::start(&path, on_view, on_stop)?);
        }
        self.web.as_ref().context(NO_HELPER)
    }

    /// ck-web has stopped (with the last thing it said): a post on its way through it fails.
    pub(super) fn on_helper_stop(&mut self, said: Option<String>) {
        self.web = None;
        // A post on its way is in the open box: closing the box stops it.
        if let Some(c) = self.reply_box().filter(|c| c.busy()) {
            c.problem(said.map_or_else(|| "ck-web stopped".to_string(), |s| format!("ck-web stopped: {s}")));
        }
    }

    /// The browser view, while the site wants a person: shown in the box.
    pub(super) fn on_view(&mut self, view: Reply) {
        let Reply::View { png, left, top } = view else { return };
        let Some(c) = self.reply_box().filter(|c| matches!(c.stage, Stage::Working | Stage::Person(_))) else { return };
        let img = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, png).ok().and_then(|b| image::load_from_memory(&b).ok());
        // The pointer starts in the middle of what's shown, and stays where it was on the page
        // as the view changes.
        if c.pointer.is_none()
            && let Some(i) = &img
        {
            c.pointer = Some((left + i.width() / 2, top + i.height() / 2));
        }
        c.stage = Stage::Person(img.map(|i| (i, left, top)));
        c.frames += 1;
    }

    /// The post's up: it's yours (the thread's watched for replies to it), and the thread's
    /// shown with it.
    pub(super) fn posted(&mut self, to: &Where, thread: u64, no: u64) {
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
        let now = self.clock.instant();
        let open_at = (!open && to.new_thread()).then(|| now + NEW_THREAD_AFTER);
        self.awaiting = Some(Awaiting { key, no, until: now + AWAIT, open_at });
        let shows = if open_at.is_some() { "it opens in a few seconds" } else { "it shows at the next refresh" };
        self.info(format!("Posted No.{no}, marked as yours; {shows}"));
    }

    /// Whether the open thread's refreshes are for a post just sent: then as often as the API
    /// allows, past its cache's `MIN_REFETCH` (as `cached_get` counts it).
    pub(super) fn awaiting_here(&self) -> bool {
        let now = self.clock.instant();
        self.awaiting.as_ref().is_some_and(|a| now < a.until && a.open_at.is_none() && self.tab.thread.as_ref().is_some_and(|t| t.key() == &a.key))
    }

    /// Open a new thread just posted once it's due; and once the thread shows the post,
    /// select it.
    pub(super) fn await_post(&mut self) {
        let now = self.clock.instant();
        let Some(a) = &mut self.awaiting else { return };
        if now >= a.until {
            self.awaiting = None;
            return;
        }
        if a.open_at.is_some_and(|t| now >= t) {
            a.open_at = None;
            let key = a.key.clone();
            self.open_key(&key);
            return;
        }
        let (key, no) = (a.key.clone(), a.no);
        if self.tab.view() == View::Thread
            && let Some(t) = self.tab.thread.as_mut().filter(|t| t.key() == &key)
            && t.select_post(no)
        {
            self.awaiting = None;
            self.info(format!("Your post No.{no} is in"));
        }
    }

    /// A 404 for a thread just posted: not on the API yet, so it's asked for again later
    /// (rather than taken as gone).
    pub(super) fn not_there_yet(&mut self, key: &ThreadKey) -> bool {
        let now = self.clock.instant();
        let Some(a) = self.awaiting.as_mut().filter(|a| &a.key == key && now < a.until) else { return false };
        a.open_at = Some(now + crate::http::MIN_REFETCH + Duration::from_secs(1));
        self.info("4chan's API doesn't list the new thread yet; trying again");
        true
    }

    /// A click on the reply box: on the browser view, to the page.
    pub(super) fn on_reply_click(&mut self, ev: MouseEvent, _: Instant) {
        if let Some((x, y)) = self.view_point(ev) {
            if let Some(c) = self.reply_box() {
                c.pointer = Some((x, y));
            }
            self.click_page(x, y);
        }
    }

    fn click_page(&mut self, x: u32, y: u32) {
        if let Some(h) = &self.web {
            let _ = h.send(&Request::Click { x, y });
        }
    }

    /// The wheel over the browser view scrolls the page.
    pub(super) fn on_reply_wheel(&mut self, ev: MouseEvent, _: Instant) {
        let dy = if ev.kind == ratatui::crossterm::event::MouseEventKind::ScrollDown { 60 } else { -60 };
        if let Some((x, y)) = self.view_point(ev)
            && let Some(h) = &self.web
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

/// What a post on its way asks of the person.
pub enum Asked {
    Solve(Challenge),
    Wait { until: Instant, message: String },
}

/// The reply box, as the post on its way asks it things: each question goes to the box, and
/// the post waits for its answer.
struct BoxAsk {
    later: Later,
    to: Where,
    attempt: u64,
}

impl BoxAsk {
    fn ask(&self, asked: Asked) -> anyhow::Result<Option<String>> {
        let (tx, rx) = channel();
        let (to, attempt) = (self.to.clone(), self.attempt);
        if !self.later.run(move |app| app.asked(&to, attempt, asked, tx)) {
            anyhow::bail!("ck is closing");
        }
        rx.recv().map_err(|_| anyhow::anyhow!("Stopped"))
    }
}

impl Ask for BoxAsk {
    fn solve(&self, challenge: &Challenge) -> anyhow::Result<Option<String>> {
        self.ask(Asked::Solve(challenge.clone()))
    }

    fn wait(&self, until: Instant, message: &str) -> anyhow::Result<()> {
        self.ask(Asked::Wait { until, message: message.into() }).map(|_| ())
    }
}

/// A captcha's small picture made bigger (a whole number of times, to at least 200 pixels
/// high): easier to make out, and drawn over enough rows to sit where it's put.
fn enlarged(img: DynamicImage) -> DynamicImage {
    let times = 200u32.div_ceil(img.height().max(1));
    if times < 2 {
        return img;
    }
    img.resize(img.width().saturating_mul(times), img.height().saturating_mul(times), image::imageops::FilterType::Lanczos3)
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
