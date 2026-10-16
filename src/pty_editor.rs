//! The user's editor ($VISUAL, else $EDITOR, else vi) in the reply box: it runs in a
//! pseudo-terminal of its own, on a file holding the comment, and its screen is drawn where
//! the comment was. Keys go to it till it quits; what it saved is the comment then.

use std::cell::Cell;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use anyhow::{Context as _, Result};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The size it starts at, till the box is drawn and says (rows, columns).
pub const FIRST_SIZE: (u16, u16) = (20, 80);

pub struct PtyEditor {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// Its screen, as what it wrote left it (the reader thread writes, drawing reads).
    screen: Arc<Mutex<vt100::Parser>>,
    /// Rows and columns, as last sized.
    size: Cell<(u16, u16)>,
    /// The comment's file, removed once read (or the box is gone).
    path: PathBuf,
}

impl PtyEditor {
    /// The user's editor on `text`. `drew` is called (on a thread of its own) each time it
    /// draws, `quit` once it quits, with whether it said it went well (`:cq` doesn't).
    pub fn start(text: &str, drew: impl Fn() + Send + 'static, quit: impl FnOnce(bool) + Send + 'static) -> Result<Self> {
        Self::start_with(&user_editor(), text, drew, quit)
    }

    fn start_with(editor: &str, text: &str, drew: impl Fn() + Send + 'static, quit: impl FnOnce(bool) + Send + 'static) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!("ck-post-{}-{}.txt", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        crate::atomic::write(&path, text.as_bytes())?;
        let (rows, cols) = FIRST_SIZE;
        let pair = portable_pty::native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
        let mut child = pair.slave.spawn_command(command(editor, &path)).with_context(|| format!("starting {editor}"))?;
        // Only the editor holds its end: when it quits, reading ours ends.
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let killer = child.clone_killer();
        let screen = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
        let shown = screen.clone();
        std::thread::spawn(move || {
            let mut buf = [0; 8192];
            while let Ok(n) = reader.read(&mut buf)
                && n > 0
            {
                lock(&shown).process(buf.get(..n).unwrap_or_default());
                drew();
            }
        });
        // Waited on apart from the reading: on Windows that ends only when ck lets go.
        std::thread::spawn(move || quit(child.wait().is_ok_and(|s| s.success())));
        Ok(PtyEditor { master: pair.master, writer, killer, screen, size: Cell::new((rows, cols)), path })
    }

    /// Its screen, to draw.
    pub fn screen(&self) -> MutexGuard<'_, vt100::Parser> {
        lock(&self.screen)
    }

    /// Fit it to `rows` by `cols`, as drawn: it's told, and draws again.
    pub fn fit(&self, rows: u16, cols: u16) {
        if self.size.get() == (rows, cols) || rows == 0 || cols == 0 {
            return;
        }
        self.size.set((rows, cols));
        self.screen().screen_mut().set_size(rows, cols);
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
    }

    /// A key, as a terminal would send it.
    pub fn key(&mut self, key: KeyEvent) {
        let bytes = key_bytes(key, self.screen().screen().application_cursor());
        let _ = self.writer.write_all(&bytes).and_then(|()| self.writer.flush());
    }

    /// Pasted text, marked as a paste when the editor asked for that.
    pub fn paste(&mut self, text: &str) {
        let text = if self.screen().screen().bracketed_paste() { format!("\x1b[200~{text}\x1b[201~") } else { text.to_string() };
        let _ = self.writer.write_all(text.as_bytes()).and_then(|()| self.writer.flush());
    }

    /// What it saved, once it's quit: None if it said it went wrong (`:cq`), which keeps
    /// the comment as it was, as git does with a commit message.
    pub fn saved(self, ok: bool) -> Result<Option<String>> {
        if !ok {
            return Ok(None);
        }
        let saved = std::fs::read_to_string(&self.path).with_context(|| format!("reading {}", self.path.display()))?;
        // An editor ends the file with a line break; the post doesn't.
        Ok(Some(saved.trim_end_matches(['\n', '\r']).to_string()))
    }
}

impl Drop for PtyEditor {
    /// A box closed under it (ck quitting) ends it, and its file goes.
    fn drop(&mut self) {
        let _ = self.killer.kill();
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A lock that a panic elsewhere doesn't make useless: the screen is only ever drawn.
fn lock(screen: &Mutex<vt100::Parser>) -> MutexGuard<'_, vt100::Parser> {
    screen.lock().unwrap_or_else(PoisonError::into_inner)
}

fn user_editor() -> String {
    ["VISUAL", "EDITOR"].iter().find_map(|v| std::env::var(v).ok().filter(|e| !e.trim().is_empty())).unwrap_or_else(|| if cfg!(windows) { "notepad" } else { "vi" }.to_string())
}

/// The editor on `path`. On Unix through the shell, as git does, so an editor set with
/// arguments ("code --wait") works; on Windows the program, in quotes if its path has
/// spaces ("C:\Program Files\…\Code.exe" --wait), then its arguments.
fn command(editor: &str, path: &std::path::Path) -> CommandBuilder {
    #[cfg(unix)]
    let mut cmd = {
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", &format!("{editor} \"$1\""), "sh"]);
        cmd.arg(path);
        cmd
    };
    #[cfg(not(unix))]
    let mut cmd = {
        let editor = editor.trim();
        let (program, rest) = match editor.strip_prefix('"').and_then(|e| e.split_once('"')) {
            Some(quoted) => quoted,
            None => editor.split_once(' ').unwrap_or((editor, "")),
        };
        let mut cmd = CommandBuilder::new(program);
        cmd.args(rest.split_whitespace());
        cmd.arg(path);
        cmd
    };
    // What vt100 draws: xterm's colors and keys.
    cmd.env("TERM", "xterm-256color");
    if let Ok(dir) = std::env::current_dir() {
        cmd.cwd(dir);
    }
    cmd
}

/// What a terminal sends for `key` (xterm's): `app_cursor` when the editor has asked for
/// the arrows' other form.
fn key_bytes(key: KeyEvent, app_cursor: bool) -> Vec<u8> {
    let (shift, alt, ctrl) = (key.modifiers.contains(KeyModifiers::SHIFT), key.modifiers.contains(KeyModifiers::ALT), key.modifiers.contains(KeyModifiers::CONTROL));
    // xterm's modifier number: 1, and 1 for shift, 2 for alt, 4 for ctrl.
    let m = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let arrow = |end: char| match (m, app_cursor) {
        (1, false) => format!("\x1b[{end}"),
        (1, true) => format!("\x1bO{end}"),
        _ => format!("\x1b[1;{m}{end}"),
    };
    let tilde = |n: u8| if m == 1 { format!("\x1b[{n}~") } else { format!("\x1b[{n};{m}~") };
    let plain = match key.code {
        KeyCode::Char(c) if ctrl => match c.to_ascii_lowercase() {
            c @ 'a'..='z' => char::from(c as u8 - b'a' + 1).to_string(),
            '@' | ' ' | '2' => "\0".into(),
            '[' | '3' => "\x1b".into(),
            '\\' | '4' => "\x1c".into(),
            ']' | '5' => "\x1d".into(),
            '^' | '6' => "\x1e".into(),
            '_' | '/' | '7' => "\x1f".into(),
            c => c.to_string(),
        },
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "\r".into(),
        KeyCode::Tab => "\t".into(),
        KeyCode::BackTab => "\x1b[Z".into(),
        KeyCode::Backspace => "\x7f".into(),
        KeyCode::Esc => "\x1b".into(),
        KeyCode::Up => return arrow('A').into_bytes(),
        KeyCode::Down => return arrow('B').into_bytes(),
        KeyCode::Right => return arrow('C').into_bytes(),
        KeyCode::Left => return arrow('D').into_bytes(),
        KeyCode::Home => return arrow('H').into_bytes(),
        KeyCode::End => return arrow('F').into_bytes(),
        KeyCode::Insert => return tilde(2).into_bytes(),
        KeyCode::Delete => return tilde(3).into_bytes(),
        KeyCode::PageUp => return tilde(5).into_bytes(),
        KeyCode::PageDown => return tilde(6).into_bytes(),
        KeyCode::F(n @ 1..=4) => {
            let end = char::from(b'P' + n - 1);
            return if m == 1 { format!("\x1bO{end}") } else { format!("\x1b[1;{m}{end}") }.into_bytes();
        }
        KeyCode::F(n @ 5..=12) => return tilde([15, 17, 18, 19, 20, 21, 23, 24].get(usize::from(n - 5)).copied().unwrap_or(15)).into_bytes(),
        _ => return Vec::new(),
    };
    // Alt goes before the key, as escape.
    if alt { format!("\x1b{plain}") } else { plain }.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_as_xterm_sends_them() {
        let k = |code, mods| key_bytes(KeyEvent::new(code, mods), false);
        assert_eq!(k(KeyCode::Char('a'), KeyModifiers::NONE), b"a");
        assert_eq!(k(KeyCode::Char('w'), KeyModifiers::CONTROL), b"\x17");
        assert_eq!(k(KeyCode::Char('x'), KeyModifiers::ALT), b"\x1bx");
        assert_eq!(k(KeyCode::Esc, KeyModifiers::NONE), b"\x1b");
        assert_eq!(k(KeyCode::Up, KeyModifiers::NONE), b"\x1b[A");
        assert_eq!(k(KeyCode::Left, KeyModifiers::CONTROL), b"\x1b[1;5D");
        assert_eq!(k(KeyCode::Delete, KeyModifiers::NONE), b"\x1b[3~");
        assert_eq!(k(KeyCode::F(5), KeyModifiers::NONE), b"\x1b[15~");
        assert_eq!(key_bytes(KeyEvent::from(KeyCode::Down), true), b"\x1bOB");
    }

    // A shell line stands in for the editor: it's given the file as $1.
    #[cfg(unix)]
    fn edit(editor: &str, text: &str) -> Option<String> {
        use std::sync::mpsc::channel;
        use std::time::Duration;
        let (tx, rx) = channel();
        let pty = PtyEditor::start_with(editor, text, || {}, move |ok| tx.send(ok).unwrap()).unwrap();
        let ok = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        pty.saved(ok).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn what_the_editor_saved_is_the_comment() {
        let edited = edit(r#"f() { printf '%s\nwritten in the editor\n\n' "$(cat "$1")" > "$1"; }; f"#, ">>123\nhi");
        assert_eq!(edited.as_deref(), Some(">>123\nhi\nwritten in the editor"));
    }

    #[cfg(unix)]
    #[test]
    fn an_editor_that_fails_keeps_the_comment() {
        assert_eq!(edit("false", "hi"), None);
    }
}
