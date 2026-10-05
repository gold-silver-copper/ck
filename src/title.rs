//! The terminal's title (OSC 2, written as the clipboard's OSC 52 is): where ck is and
//! what's new, for a terminal tab or window in the background. The title from before is
//! pushed on the terminal's title stack first and popped back on exit; terminals without
//! the stack are left with an empty title instead of ck's.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

/// The title from before ck was pushed (so there's something to pop).
static PUSHED: AtomicBool = AtomicBool::new(false);

/// The longest title written, in characters.
const MAX: usize = 120;

/// Set the terminal's title.
pub fn set(text: &str) {
    if crate::sandboxed() {
        return;
    }
    // The first time, xterm's "push window title" first.
    let push = if PUSHED.swap(true, Ordering::SeqCst) { "" } else { "\x1b[22;2t" };
    write(&format!("{push}\x1b]2;{}\x07", clean(text)));
}

/// Put back the title from before ck (or clear ck's), if ck set one.
pub fn restore() {
    if crate::sandboxed() || !PUSHED.swap(false, Ordering::SeqCst) {
        return;
    }
    write("\x1b]2;\x07\x1b[23;2t");
}

fn write(s: &str) {
    let mut out = std::io::stdout();
    let _ = out.write_all(s.as_bytes()).and_then(|()| out.flush());
}

/// A title as safe to write inside an escape sequence: a thread's subject is the site's
/// text, and a control character in it (ESC, BEL, C1's ST and CSI) would end the sequence
/// and send the rest to the terminal as commands. Those become spaces; bidi overrides go
/// too, and so does what's past `MAX`.
pub fn clean(text: &str) -> String {
    let text = crate::markup::for_terminal(text);
    let spaced: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut out = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((cut, _)) = out.char_indices().nth(MAX) {
        out.truncate(cut);
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn titles_carry_no_escapes() {
        let evil = "a\x1b]2;pwned\x07b\x1b[2Jc\u{9b}31m\u{9c}d\re\nf\u{202e}gpj.exe\u{7f}";
        let t = clean(evil);
        assert!(!t.chars().any(char::is_control), "{t:?}");
        assert_eq!(t, "a ]2;pwned b [2Jc 31m d e fgpj.exe");
        assert_eq!(clean("  plain   words "), "plain words");
        let long = clean(&"é".repeat(500));
        assert_eq!(long.chars().count(), super::MAX + 1);
        assert!(long.ends_with('…'));
    }
}
