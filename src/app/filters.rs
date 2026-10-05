//! Filters from where you are: `X` (or the `.` menu) hides or highlights posts like the
//! selected one, and Settings › Filters lists, edits, turns off and removes them. Every
//! change is written to the config's `[[filter]]` tables, keeping the rest of the file.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::ListState;

use super::settings::SettingsPopup;
use super::{App, Part, Popup, View, edit_text, list_move};
use crate::config::FilterEdit;
use crate::filter::{Field, FilterAction, FilterConfig, Filters};
use crate::model::Post;

/// Where a filter applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    Board,
    Site,
    Everywhere,
}

impl Reach {
    fn next(self) -> Self {
        match self {
            Reach::Board => Reach::Site,
            Reach::Site => Reach::Everywhere,
            Reach::Everywhere => Reach::Board,
        }
    }
}

/// A filter that would catch the selected post, by one thing about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// What it catches, in words: "posts by Anonymous !trip".
    pub what: String,
    pub field: Field,
    pub pattern: String,
    pub label: String,
}

/// What `u` right after takes back: a filter added (at its place), or a hidden word.
pub enum Undo {
    Filter(usize, FilterConfig),
    Word(String),
}

/// `X`: a filter like the selected post, being made.
pub struct AddFilter {
    /// The post (or catalog thread) it's made from.
    pub post: u64,
    pub site: String,
    pub board: String,
    pub candidates: Vec<Candidate>,
    pub list: ListState,
    pub action: FilterAction,
    pub reach: Reach,
    /// The label, once changed by hand; `typing` while it's being edited.
    pub label: Option<String>,
    pub typing: Option<String>,
    /// `w`: a word from it to hide everywhere, being typed.
    pub word: Option<String>,
}

impl AddFilter {
    pub fn candidate(&self) -> Option<&Candidate> {
        self.list.selected().and_then(|i| self.candidates.get(i))
    }

    pub fn label(&self) -> String {
        self.label.clone().or_else(|| self.candidate().map(|c| c.label.clone())).unwrap_or_default()
    }

    pub fn reach_text(&self) -> String {
        match self.reach {
            Reach::Board => format!("/{}/ on {}", self.board, self.site),
            Reach::Site => format!("all of {}", self.site),
            Reach::Everywhere => "every site".into(),
        }
    }

    /// The filter as it would be added.
    pub fn config(&self) -> Option<FilterConfig> {
        let c = self.candidate()?;
        let mut f = FilterConfig::new(c.pattern.clone(), &[c.field]);
        f.action = self.action;
        f.label = Some(self.label()).filter(|l| !l.is_empty());
        match self.reach {
            Reach::Board => {
                f.sites = vec![self.site.clone()];
                f.boards = vec![self.board.clone()];
            }
            Reach::Site => f.sites = vec![self.site.clone()],
            Reach::Everywhere => {}
        }
        Some(f)
    }
}

/// Names sites give everyone who doesn't give one (filtering by them would hide everyone).
const ANONYMOUS: &[&str] = &["anonymous", "anon", "аноним", "anonyme", "anónimo", "anonimo", "名無しさん", "名無し", "nameless"];

/// The filters that would catch `p`: by its name (with the tripcode), its file's MD5 or
/// name (the focused file, else the first), and an OP's subject. `default_name` is the name
/// most posts around it have, which is skipped like the usual anonymous names.
pub fn candidates(p: &Post, file: Option<usize>, is_op: bool, default_name: Option<&str>) -> Vec<Candidate> {
    let mut out = Vec::new();
    let name = p.name.trim();
    let anonymous = name.is_empty() || ANONYMOUS.contains(&name.to_lowercase().as_str()) || default_name == Some(p.name.as_str());
    if !anonymous {
        out.push(Candidate {
            what: format!("posts by {}", crate::ui::truncate(name, 40)),
            field: Field::Name,
            pattern: format!("^{}$", regex::escape(&p.name)),
            label: name.to_string(),
        });
    }
    let focused = file.and_then(|k| p.files.get(k));
    let with_md5 = focused.filter(|f| f.md5.is_some()).or_else(|| p.files.iter().find(|f| f.md5.is_some()));
    if let Some(md5) = with_md5.and_then(|f| f.md5.clone()) {
        let short: String = md5.chars().take(8).collect();
        out.push(Candidate { what: "posts with this image".into(), field: Field::Md5, pattern: md5, label: format!("image {short}") });
    }
    if let Some(f) = focused.or(p.files.first()).filter(|f| !f.filename.is_empty()) {
        let stem = f.filename.rsplit_once('.').map(|(s, _)| s).filter(|s| !s.is_empty()).unwrap_or(&f.filename);
        out.push(Candidate {
            what: format!("files named like {}", crate::ui::truncate(stem, 40)),
            field: Field::Filename,
            pattern: format!("^{}(\\.[^.]*)?$", regex::escape(stem)),
            label: format!("file {}", crate::ui::truncate(stem, 30)),
        });
    }
    if let Some(subject) = p.subject.as_deref().filter(|s| is_op && !s.trim().is_empty()) {
        out.push(Candidate {
            what: format!("threads titled {}", crate::ui::truncate(subject, 40)),
            field: Field::Subject,
            pattern: format!("^{}$", regex::escape(subject)),
            label: crate::ui::truncate(subject, 40),
        });
    }
    out
}

/// The name most of `posts` have, if most have one (a site's own default name).
fn usual_name(posts: &[Post]) -> Option<String> {
    let mut counts: std::collections::HashMap<&str, usize> = Default::default();
    for p in posts {
        *counts.entry(p.name.as_str()).or_default() += 1;
    }
    let (name, n) = counts.into_iter().max_by_key(|&(name, n)| (n, std::cmp::Reverse(name)))?;
    (posts.len() >= 4 && n * 2 > posts.len()).then(|| name.to_string())
}

/// The rows of the filter editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditRow {
    Pattern,
    Label,
    Action,
    Field(Field),
    Sites,
    Boards,
    Enabled,
}

pub const EDIT_ROWS: [EditRow; 11] = [
    EditRow::Pattern,
    EditRow::Label,
    EditRow::Action,
    EditRow::Field(Field::Subject),
    EditRow::Field(Field::Comment),
    EditRow::Field(Field::Name),
    EditRow::Field(Field::Filename),
    EditRow::Field(Field::Md5),
    EditRow::Sites,
    EditRow::Boards,
    EditRow::Enabled,
];

impl EditRow {
    pub fn is_text(self) -> bool {
        matches!(self, EditRow::Pattern | EditRow::Label | EditRow::Sites | EditRow::Boards)
    }

    /// The row's value as text to edit.
    pub fn text(self, f: &FilterConfig) -> String {
        match self {
            EditRow::Pattern => f.pattern.clone(),
            EditRow::Label => f.label.clone().unwrap_or_default(),
            EditRow::Sites => f.sites.join(", "),
            EditRow::Boards => f.boards.join(", "),
            _ => String::new(),
        }
    }
}

/// `a, b c` as a list.
fn words(s: &str) -> Vec<String> {
    s.split([',', ' ']).map(str::trim).filter(|w| !w.is_empty()).map(String::from).collect()
}

/// `draft` with a text row set to `text`.
pub fn with_text(draft: &FilterConfig, row: EditRow, text: &str) -> FilterConfig {
    let mut f = draft.clone();
    match row {
        EditRow::Pattern => f.pattern = text.to_string(),
        EditRow::Label => f.label = Some(text.trim().to_string()).filter(|l| !l.is_empty()),
        EditRow::Sites => f.sites = words(text),
        EditRow::Boards => f.boards = words(text).into_iter().map(|b| b.trim_matches('/').to_string()).collect(),
        _ => {}
    }
    f
}

/// Why a draft can't be saved, if it can't.
pub fn problem(f: &FilterConfig) -> Option<String> {
    if f.pattern.is_empty() {
        return Some("A filter needs a pattern".into());
    }
    // A regex error is several lines, pointing at the spot; its last line says what's wrong.
    let e = f.check().err()?.root_cause().to_string();
    let what = e.lines().rev().find_map(|l| l.trim().strip_prefix("error: ")).unwrap_or(e.lines().next().unwrap_or_default());
    Some(format!("Not a valid pattern: {what}"))
}

/// The post `X` makes a filter from.
struct Source<'a> {
    post: &'a Post,
    board: String,
    /// The focused file.
    file: Option<usize>,
    is_op: bool,
    /// The name most posts around it have.
    usual: Option<String>,
}

impl App {
    /// The post `X` makes a filter from: in a thread the selected post (and its focused
    /// file), in a catalog the selected thread.
    fn filter_source(&self) -> Option<Source<'_>> {
        match self.tab.view {
            View::Thread => {
                let t = self.tab.thread.as_ref()?;
                let p = t.current()?;
                let file = match &t.focus {
                    Some(Part::File(k)) => Some(*k),
                    _ => None,
                };
                Some(Source { post: p, board: t.board.clone(), file, is_op: t.selected == 0, usual: usual_name(&t.posts) })
            }
            View::Catalog => {
                let p = self.selected_post()?;
                Some(Source { post: p, board: self.board_of(p), file: None, is_op: true, usual: usual_name(&self.tab.catalog) })
            }
            _ => None,
        }
    }

    /// `X`: hide or highlight posts like the selected one.
    pub fn open_add_filter(&mut self) {
        let Some(Source { post: p, board, file, is_op, usual }) = self.filter_source() else { return };
        let candidates = candidates(p, file, is_op, usual.as_deref());
        // Nothing else to go by: a word from it, straight away.
        let word = candidates.is_empty().then(|| self.tab.thread.as_ref().map(|t| t.search.clone()).unwrap_or_default());
        // A focused file: its MD5 first.
        let first = if file.is_some() { candidates.iter().position(|c| c.field == Field::Md5).unwrap_or(0) } else { 0 };
        self.popup = Some(Popup::AddFilter(AddFilter {
            post: p.no,
            site: self.current_site().cfg.name.clone(),
            board,
            candidates,
            list: ListState::default().with_selected(Some(first)),
            action: FilterAction::Hide,
            reach: Reach::Board,
            label: None,
            typing: None,
            word,
        }));
    }

    pub fn on_add_filter_key(&mut self, key: KeyEvent) {
        let Some(mut a) = take_popup!(self, AddFilter) else { return };
        if let Some(mut word) = a.word.take() {
            match key.code {
                // Back to the choices, or out when there are none.
                KeyCode::Esc if a.candidates.is_empty() => return,
                KeyCode::Esc => {}
                KeyCode::Enter => return self.add_hidden_word(&word),
                code => {
                    edit_text(&mut word, code);
                    a.word = Some(word);
                }
            }
            self.popup = Some(Popup::AddFilter(a));
            return;
        }
        if let Some(mut text) = a.typing.take() {
            match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => a.label = Some(text.trim().to_string()),
                code => {
                    edit_text(&mut text, code);
                    a.typing = Some(text);
                }
            }
            self.popup = Some(Popup::AddFilter(a));
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return,
            KeyCode::Enter => {
                if let Some(f) = a.config() {
                    self.add_filter(f);
                }
                return;
            }
            KeyCode::Char('a' | 'h' | 'l' | ' ') | KeyCode::Left | KeyCode::Right => {
                a.action = match a.action {
                    FilterAction::Hide => FilterAction::Highlight,
                    FilterAction::Highlight => FilterAction::Hide,
                };
            }
            KeyCode::Char('s') => a.reach = a.reach.next(),
            KeyCode::Char('e') => a.typing = Some(a.label()),
            // A word from it: the thread's search, if there's one, to start with.
            KeyCode::Char('w') => a.word = Some(self.tab.thread.as_ref().map(|t| t.search.clone()).unwrap_or_default()),
            code => {
                let cur = a.list.selected().unwrap_or(0);
                if let Some(to) = list_move(code, cur, a.candidates.len()) {
                    a.list.select(Some(to));
                    // The label follows the choice until it's changed by hand.
                }
            }
        }
        self.popup = Some(Popup::AddFilter(a));
    }

    /// Add a filter; `u` right after takes it back.
    fn add_filter(&mut self, f: FilterConfig) {
        if let Some(e) = problem(&f) {
            self.error(e);
            return;
        }
        let saved = self.write_filters(FilterEdit::Add(&f));
        self.filter_cfgs.push(f.clone());
        self.apply_filters();
        let (posts, threads) = self.filter_counts(&f);
        let verb = match f.action {
            FilterAction::Hide => "Hiding",
            FilterAction::Highlight => "Highlighting",
        };
        let here = match (posts, threads) {
            (0, 0) => String::new(),
            (p, 0) => format!(" ({p} here)"),
            (0, t) => format!(" ({t} here)"),
            (p, t) => format!(" ({p} posts, {t} threads here)"),
        };
        let note = match saved {
            Ok(()) => "u undoes; Settings › Filters lists them".to_string(),
            Err(e) => format!("u undoes; not saved: {e:#}"),
        };
        self.info(format!("{verb} {}{here} · {note}", f.label()));
        self.filter_undo = Some(Undo::Filter(self.filter_cfgs.len() - 1, f));
    }

    /// Hide posts with `word`, everywhere; `u` right after takes it back.
    pub(super) fn add_hidden_word(&mut self, word: &str) {
        let word = word.split_whitespace().collect::<Vec<_>>().join(" ");
        if crate::filter::word_pattern(&word).is_none() {
            return self.error("A hidden word needs a letter or two");
        }
        if self.hidden_words.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
            return self.info(format!("\"{word}\" is already hidden"));
        }
        self.hidden_words.push(word.clone());
        let saved = self.write_hidden_words();
        self.apply_filters();
        let label = format!("hidden word: {word}");
        let posts = self.tab.thread.as_ref().map_or(0, |t| t.marks.iter().filter(|m| m.hidden.as_deref() == Some(&label)).count());
        let threads = self.tab.catalog_marks.iter().filter(|m| m.hidden.as_deref() == Some(&label)).count();
        let here = match (posts, threads) {
            (0, 0) => String::new(),
            (p, 0) => format!(" ({p} here)"),
            (0, t) => format!(" ({t} here)"),
            (p, t) => format!(" ({p} posts, {t} threads here)"),
        };
        let note = match saved {
            Ok(()) => "u undoes; Settings › Filters › Hidden words lists them".to_string(),
            Err(e) => format!("u undoes; not saved: {e:#}"),
        };
        self.info(format!("Hiding posts with \"{word}\"{here} · {note}"));
        self.filter_undo = Some(Undo::Word(word));
    }

    /// Stop hiding posts with a word.
    pub(super) fn remove_hidden_word(&mut self, word: &str) {
        let before = self.hidden_words.len();
        self.hidden_words.retain(|w| w != word);
        if self.hidden_words.len() == before {
            return;
        }
        let saved = self.write_hidden_words();
        self.apply_filters();
        match saved {
            Ok(()) => self.info(format!("Posts with \"{word}\" aren't hidden now")),
            Err(e) => self.error(format!("Posts with \"{word}\" aren't hidden for now; couldn't save it: {e:#}")),
        }
    }

    fn write_hidden_words(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.config_path.is_some(), "no config file");
        self.edit_config(|d| {
            crate::config::set_hidden_words(d, &self.hidden_words);
            Ok(())
        })
        .map(drop)
    }

    /// `u` right after adding a filter or a hidden word: take it back.
    pub fn undo_filter(&mut self) {
        let (i, f) = match self.filter_undo.take() {
            Some(Undo::Filter(i, f)) => (i, f),
            Some(Undo::Word(w)) => return self.remove_hidden_word(&w),
            None => return,
        };
        if self.filter_cfgs.get(i) != Some(&f) {
            return;
        }
        let saved = self.write_filters(FilterEdit::Remove(i, &f));
        self.filter_cfgs.remove(i);
        self.apply_filters();
        match saved {
            Ok(()) => self.info(format!("Took the filter {} back", f.label())),
            Err(e) => self.error(format!("Took the filter {} back for now; couldn't save it: {e:#}", f.label())),
        }
    }

    /// Write a change to the config's filters. Without a config file to write (or when the
    /// file's filters changed meanwhile), the change still holds until ck quits.
    fn write_filters(&self, edit: FilterEdit) -> anyhow::Result<()> {
        anyhow::ensure!(self.config_path.is_some(), "no config file");
        self.edit_config(|d| crate::config::edit_filters(d, edit)).map(drop)
    }

    /// The filters changed: rebuild them, and mark the catalog and thread again.
    pub(super) fn apply_filters(&mut self) {
        match Filters::from_config(&self.filter_cfgs, &self.hidden_words) {
            Ok(f) => self.filters = f,
            Err(e) => self.error(format!("{e:#}")),
        }
        self.remark_catalog();
        self.remark_thread();
        let len = self.visible_catalog().len();
        self.tab.catalog_list.clamp(len);
    }

    /// How many posts of the open thread, and threads of the open catalog, `f` catches
    /// (whether it's on or not).
    pub fn filter_counts(&self, f: &FilterConfig) -> (usize, usize) {
        let Ok(one) = Filters::new(std::slice::from_ref(&FilterConfig { enabled: true, ..f.clone() })) else { return (0, 0) };
        let site = &self.current_site().cfg.name;
        let caught = |board: &str, p: &Post| {
            let m = one.check(site, board, p);
            m.hidden.is_some() || m.highlight.is_some()
        };
        let posts = self.tab.thread.as_ref().map_or(0, |t| t.posts.iter().filter(|p| caught(&t.board, p)).count());
        let threads = self.tab.catalog.iter().filter(|p| caught(&self.board_of(p), p)).count();
        (posts, threads)
    }

    // ----- Settings › Filters -----

    pub fn filter_list(&self, selected: usize) -> SettingsPopup {
        let counts = self.filter_cfgs.iter().map(|f| self.filter_counts(f)).collect();
        let sel = (!self.filter_cfgs.is_empty()).then(|| selected.min(self.filter_cfgs.len() - 1));
        SettingsPopup::Filters { list: ListState::default().with_selected(sel), counts }
    }

    /// Keys in the list of filters.
    pub(super) fn on_filter_list_key(&mut self, key: KeyEvent, mut list: ListState, counts: Vec<(usize, usize)>) -> Option<SettingsPopup> {
        let sel = list.selected();
        let editor = |index: Option<usize>, draft: FilterConfig, typing: Option<String>| SettingsPopup::FilterEdit { index, draft, row: 0, typing };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'h') | KeyCode::Left => None,
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => match sel.and_then(|i| self.filter_cfgs.get(i).cloned().map(|f| (i, f))) {
                Some((i, f)) => Some(editor(Some(i), f, None)),
                None => Some(SettingsPopup::Filters { list, counts }),
            },
            KeyCode::Char('a') => {
                // Typing its pattern right away.
                Some(editor(None, FilterConfig::new(String::new(), &[Field::Subject, Field::Comment]), Some(String::new())))
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                if let Some(i) = sel.filter(|&i| i < self.filter_cfgs.len()) {
                    let f = self.filter_cfgs[i].clone();
                    let saved = self.write_filters(FilterEdit::Remove(i, &f));
                    self.filter_cfgs.remove(i);
                    self.apply_filters();
                    self.say_saved(&format!("Removed the filter {}", f.label()), saved);
                }
                Some(self.filter_list(sel.unwrap_or(0)))
            }
            KeyCode::Char(' ') => {
                if let Some(i) = sel.filter(|&i| i < self.filter_cfgs.len()) {
                    let old = self.filter_cfgs[i].clone();
                    let new = FilterConfig { enabled: !old.enabled, ..old.clone() };
                    let what = format!("{} the filter {}", if new.enabled { "Turned on" } else { "Turned off" }, new.label());
                    let saved = self.write_filters(FilterEdit::Change(i, &old, &new));
                    self.filter_cfgs[i] = new;
                    self.apply_filters();
                    self.say_saved(&what, saved);
                }
                Some(SettingsPopup::Filters { list, counts })
            }
            code => {
                let cur = sel.unwrap_or(0);
                list.select(list_move(code, cur, self.filter_cfgs.len()).or(sel).filter(|_| !self.filter_cfgs.is_empty()));
                Some(SettingsPopup::Filters { list, counts })
            }
        }
    }

    fn say_saved(&mut self, what: &str, saved: anyhow::Result<()>) {
        match saved {
            Ok(()) => self.info(what),
            Err(e) => self.error(format!("{what} for now; couldn't save it: {e:#}")),
        }
    }

    /// Keys in the filter editor. Each accepted change is saved at once; a new filter is
    /// saved once it has a pattern.
    pub(super) fn on_filter_edit_key(&mut self, key: KeyEvent, index: Option<usize>, draft: FilterConfig, mut row: usize, typing: Option<String>) -> Option<SettingsPopup> {
        let r = EDIT_ROWS.get(row).copied().unwrap_or(EditRow::Pattern);
        if let Some(mut text) = typing {
            return match key.code {
                // A new filter without a pattern yet is dropped.
                KeyCode::Esc if index.is_none() && draft.pattern.is_empty() => Some(self.filter_list(usize::MAX)),
                KeyCode::Esc => Some(SettingsPopup::FilterEdit { index, draft, row, typing: None }),
                KeyCode::Enter => {
                    let next = with_text(&draft, r, &text);
                    match problem(&next) {
                        Some(e) => {
                            self.error(e);
                            Some(SettingsPopup::FilterEdit { index, draft, row, typing: Some(text) })
                        }
                        None => {
                            let index = self.save_filter(index, &draft, &next);
                            Some(SettingsPopup::FilterEdit { index, draft: next, row, typing: None })
                        }
                    }
                }
                code => {
                    edit_text(&mut text, code);
                    Some(SettingsPopup::FilterEdit { index, draft, row, typing: Some(text) })
                }
            };
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'h') | KeyCode::Left => Some(self.filter_list(index.unwrap_or(usize::MAX))),
            KeyCode::Enter | KeyCode::Char(' ' | 'l') | KeyCode::Right => {
                if r.is_text() {
                    return Some(SettingsPopup::FilterEdit { index, draft: draft.clone(), row, typing: Some(r.text(&draft)) });
                }
                let mut next = draft.clone();
                match r {
                    EditRow::Action => {
                        next.action = match next.action {
                            FilterAction::Hide => FilterAction::Highlight,
                            FilterAction::Highlight => FilterAction::Hide,
                        }
                    }
                    EditRow::Enabled => next.enabled = !next.enabled,
                    EditRow::Field(field) => {
                        let mut fields = next.fields();
                        match fields.iter().position(|&f| f == field) {
                            Some(_) if fields.len() == 1 => {
                                self.error("A filter needs at least one field");
                                return Some(SettingsPopup::FilterEdit { index, draft, row, typing: None });
                            }
                            Some(k) => {
                                fields.remove(k);
                            }
                            None => fields.push(field),
                        }
                        // In the usual order.
                        fields.sort_by_key(|f| Field::ALL.iter().position(|a| a == f));
                        next.set_fields(&fields);
                    }
                    _ => {}
                }
                if let Some(e) = problem(&next).filter(|_| !next.pattern.is_empty()) {
                    self.error(e);
                    return Some(SettingsPopup::FilterEdit { index, draft, row, typing: None });
                }
                let index = self.save_filter(index, &draft, &next);
                Some(SettingsPopup::FilterEdit { index, draft: next, row, typing: None })
            }
            code => {
                row = list_move(code, row, EDIT_ROWS.len()).unwrap_or(row);
                Some(SettingsPopup::FilterEdit { index, draft, row, typing: None })
            }
        }
    }

    /// Save an edited filter (`old` is how it was); a new one is added once it has a
    /// pattern. Returns its index.
    fn save_filter(&mut self, index: Option<usize>, old: &FilterConfig, new: &FilterConfig) -> Option<usize> {
        if new == old && index.is_some() {
            return index;
        }
        let (index, saved) = match index {
            Some(i) if self.filter_cfgs.get(i) == Some(old) => {
                let saved = self.write_filters(FilterEdit::Change(i, old, new));
                self.filter_cfgs[i] = new.clone();
                (Some(i), saved)
            }
            // Gone meanwhile (it can't be, with the popup open): nothing to change.
            Some(_) => return None,
            None if new.pattern.is_empty() => return None,
            None => {
                let saved = self.write_filters(FilterEdit::Add(new));
                self.filter_cfgs.push(new.clone());
                (Some(self.filter_cfgs.len() - 1), saved)
            }
        };
        self.apply_filters();
        self.say_saved(&format!("Saved the filter {}", new.label()), saved);
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Attachment;

    fn post(name: &str) -> Post {
        let file = |name: &str, md5: Option<&str>| Attachment { filename: name.into(), md5: md5.map(String::from), ..Default::default() };
        Post {
            no: 7,
            name: name.into(),
            subject: Some("Rust (general) [42]".into()),
            files: vec![file("cat.v2.png", None), file("dog.jpg", Some("abc+/def=="))],
            ..Default::default()
        }
    }

    #[test]
    fn candidates_escape_and_anchor() {
        let c = candidates(&post("Mr. (Smith) !!Trip+"), None, true, None);
        let by = |field| c.iter().find(|c| c.field == field).unwrap();
        // The name with its tripcode, exactly.
        assert_eq!(by(Field::Name).pattern, r"^Mr\. \(Smith\) !!Trip\+$");
        // The MD5 compares as it is (not a regex): the first file that has one.
        assert_eq!((by(Field::Md5).pattern.as_str(), by(Field::Md5).label.as_str()), ("abc+/def==", "image abc+/def"));
        // The focused file's name, without its extension.
        assert_eq!(by(Field::Filename).pattern, r"^cat\.v2(\.[^.]*)?$");
        assert_eq!(by(Field::Subject).pattern, r"^Rust \(general\) \[42\]$");
        let c = candidates(&post("x"), Some(1), false, None);
        assert_eq!(c.iter().find(|c| c.field == Field::Filename).unwrap().pattern, r"^dog(\.[^.]*)?$");
        // Not an OP: no subject. Anonymous (or the name most posts have): no name.
        assert!(c.iter().all(|c| c.field != Field::Subject));
        assert!(candidates(&post("Anonymous"), None, true, None).iter().all(|c| c.field != Field::Name));
        assert!(candidates(&post("Аноним"), None, true, None).iter().all(|c| c.field != Field::Name));
        assert!(candidates(&post("Nanashi"), None, true, Some("Nanashi")).iter().all(|c| c.field != Field::Name));
        // Each one catches the post it's made from, and only by what it says.
        let p = post("Mr. (Smith) !!Trip+");
        for c in candidates(&p, None, true, None) {
            let f = Filters::new(&[FilterConfig::new(c.pattern.clone(), &[c.field])]).unwrap();
            assert!(f.check("4chan", "g", &p).hidden.is_some(), "{c:?}");
            assert!(f.check("4chan", "g", &post("Someone")).hidden.is_none() || c.field != Field::Name, "{c:?}");
        }
    }

    #[test]
    fn scope_label_and_action() {
        let mut a = AddFilter {
            post: 7,
            site: "4chan".into(),
            board: "g".into(),
            candidates: candidates(&post("Named"), None, true, None),
            list: ListState::default().with_selected(Some(0)),
            action: FilterAction::Hide,
            reach: Reach::Board,
            label: None,
            typing: None,
            word: None,
        };
        let f = a.config().unwrap();
        assert_eq!((f.sites.as_slice(), f.boards.as_slice(), f.label.as_deref()), (&["4chan".to_string()][..], &["g".to_string()][..], Some("Named")));
        a.reach = a.reach.next();
        a.action = FilterAction::Highlight;
        a.label = Some("someone".into());
        let f = a.config().unwrap();
        assert_eq!((f.sites.len(), f.boards.len(), f.action, f.label.as_deref()), (1, 0, FilterAction::Highlight, Some("someone")));
        a.reach = a.reach.next();
        let f = a.config().unwrap();
        assert!(f.sites.is_empty() && f.boards.is_empty());
    }

    #[test]
    fn editor_text_and_problems() {
        let f = FilterConfig::new("x".into(), &[Field::Name]);
        let g = with_text(&f, EditRow::Boards, "/g/, v  tech");
        assert_eq!(g.boards, ["g", "v", "tech"]);
        assert_eq!(with_text(&f, EditRow::Label, "  ").label, None);
        assert_eq!(problem(&with_text(&f, EditRow::Pattern, "(unclosed")).as_deref(), Some("Not a valid pattern: unclosed group"));
        assert_eq!(problem(&with_text(&f, EditRow::Pattern, "")).as_deref(), Some("A filter needs a pattern"));
        // An MD5 filter's pattern isn't a regex.
        assert!(problem(&FilterConfig::new("(abc".into(), &[Field::Md5])).is_none());
    }
}
