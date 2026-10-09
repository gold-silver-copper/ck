//! A text field with a cursor: one line (a name, a path) or many (a post's comment), with
//! the keys of a shell's line editor.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

use crate::markup;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Editor {
    text: String,
    /// Where the cursor is: a byte offset, always on a grapheme's start.
    cursor: usize,
    /// Enter makes a new line (else it's for the field's owner).
    pub multiline: bool,
}

impl Editor {
    pub fn multiline() -> Self {
        Editor { multiline: true, ..Self::default() }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Replace the text; the cursor goes to its end.
    pub fn set(&mut self, text: &str) {
        self.text = self.clean(text);
        self.cursor = self.text.len();
    }

    /// Type (or paste) `s` at the cursor. A one-line field takes line breaks as spaces.
    pub fn insert(&mut self, s: &str) {
        let s = self.clean(s);
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }

    fn clean(&self, s: &str) -> String {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        if self.multiline { s } else { s.replace('\n', " ") }
    }

    /// A key for the field; whether it was one (enter in a one-line field isn't).
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('a') if ctrl => self.cursor = self.line_start(),
            KeyCode::Char('e') if ctrl => self.cursor = self.line_end(),
            KeyCode::Char('w') if ctrl => self.delete_to(self.word_left()),
            KeyCode::Backspace if ctrl || alt => self.delete_to(self.word_left()),
            KeyCode::Char('u') if ctrl => self.delete_to(self.line_start()),
            KeyCode::Char('k') if ctrl => self.delete_to(self.line_end()),
            KeyCode::Char(_) if ctrl => return false,
            KeyCode::Char(c) => self.insert(c.encode_utf8(&mut [0; 4])),
            KeyCode::Enter if self.multiline => self.insert("\n"),
            KeyCode::Backspace => self.delete_to(self.prev()),
            KeyCode::Delete => self.delete_to(self.next()),
            KeyCode::Left if ctrl || alt => self.cursor = self.word_left(),
            KeyCode::Right if ctrl || alt => self.cursor = self.word_right(),
            KeyCode::Left => self.cursor = self.prev(),
            KeyCode::Right => self.cursor = self.next(),
            KeyCode::Home => self.cursor = self.line_start(),
            KeyCode::End => self.cursor = self.line_end(),
            KeyCode::Up if self.multiline => self.vertical(false),
            KeyCode::Down if self.multiline => self.vertical(true),
            _ => return false,
        }
        true
    }

    /// Delete from the cursor to `to`, either way.
    fn delete_to(&mut self, to: usize) {
        let (a, b) = (self.cursor.min(to), self.cursor.max(to));
        self.text.replace_range(a..b, "");
        self.cursor = a;
    }

    fn prev(&self) -> usize {
        self.before().grapheme_indices(true).next_back().map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.after().graphemes(true).next().map_or(self.cursor, |g| self.cursor + g.len())
    }

    fn before(&self) -> &str {
        self.text.get(..self.cursor).unwrap_or_default()
    }

    fn after(&self) -> &str {
        self.text.get(self.cursor..).unwrap_or_default()
    }

    fn line_start(&self) -> usize {
        self.before().rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.after().find('\n').map_or(self.text.len(), |i| self.cursor + i)
    }

    /// The start of the word before the cursor (past the spaces before it).
    fn word_left(&self) -> usize {
        let before = self.before();
        let end = before.trim_end().len();
        before.get(..end).and_then(|s| s.rfind(char::is_whitespace)).map_or(0, |i| i + 1)
    }

    /// The end of the word after the cursor.
    fn word_right(&self) -> usize {
        let after = self.after();
        let start = after.len() - after.trim_start().len();
        let rest = after.get(start..).unwrap_or_default();
        self.cursor + start + rest.find(char::is_whitespace).unwrap_or(rest.len())
    }

    /// Up or down a line, to the same column or the line's end.
    fn vertical(&mut self, down: bool) {
        let start = self.line_start();
        let column = markup::columns(self.text.get(start..self.cursor).unwrap_or_default());
        let target = if down {
            let end = self.line_end();
            if end >= self.text.len() {
                return;
            }
            end + 1
        } else {
            let Some(prev_end) = start.checked_sub(1) else { return };
            self.text.get(..prev_end).and_then(|s| s.rfind('\n')).map_or(0, |i| i + 1)
        };
        let line = self.text.get(target..).unwrap_or_default();
        let line = line.split('\n').next().unwrap_or_default();
        let mut at = target;
        let mut cols = 0;
        for g in line.graphemes(true) {
            let w = markup::columns(g);
            if cols + w > column {
                break;
            }
            cols += w;
            at += g.len();
        }
        self.cursor = at;
    }

    /// The text wrapped to `width` columns, and the cursor's row and column in it.
    pub fn wrapped(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(1);
        let mut rows = Vec::new();
        let mut cursor = (0, 0);
        let mut offset = 0;
        for line in self.text.split('\n') {
            let mut row = String::new();
            let mut cols = 0;
            for (i, g) in line.grapheme_indices(true) {
                let w = markup::columns(g);
                if cols + w > width && !row.is_empty() {
                    rows.push(std::mem::take(&mut row));
                    cols = 0;
                }
                if offset + i == self.cursor {
                    cursor = (rows.len(), cols);
                }
                row.push_str(g);
                cols += w;
            }
            if offset + line.len() == self.cursor {
                // At the end of a full row, the cursor starts the next.
                cursor = if cols >= width { (rows.len() + 1, 0) } else { (rows.len(), cols) };
            }
            rows.push(row);
            offset += line.len() + 1;
        }
        (rows, cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn edits_at_the_cursor() {
        let mut e = Editor::multiline();
        e.insert(">>123\nhello wörld");
        e.on_key(ctrl('w'));
        assert_eq!(e.text(), ">>123\nhello ");
        e.on_key(key(KeyCode::Up));
        e.on_key(key(KeyCode::End));
        e.on_key(key(KeyCode::Enter));
        e.insert(">quoted");
        assert_eq!(e.text(), ">>123\n>quoted\nhello ");
        e.on_key(ctrl('u'));
        assert_eq!(e.text(), ">>123\n\nhello ");
        // A one-line field turns pasted lines into one, and leaves enter to its owner.
        let mut name = Editor::default();
        name.insert("a\nb");
        assert!(!name.on_key(key(KeyCode::Enter)));
        assert_eq!(name.text(), "a b");
    }

    #[test]
    fn wraps_with_the_cursor() {
        let mut e = Editor::multiline();
        e.insert("abcdef\ngh");
        assert_eq!(e.wrapped(4), (vec!["abcd".into(), "ef".into(), "gh".into()], (2, 2)));
        e.set("abcd");
        assert_eq!(e.wrapped(4).1, (1, 0));
        e.on_key(key(KeyCode::Left));
        e.on_key(key(KeyCode::Left));
        assert_eq!(e.wrapped(3).1, (0, 2));
    }
}
