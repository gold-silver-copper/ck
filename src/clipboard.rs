//! Copying text: OSC 52 (the terminal sets the clipboard; works over SSH, and in tmux with
//! `set-clipboard on`), plus the system's clipboard command when there's one locally.

use std::io::Write;
use std::process::{Command, Stdio};

/// Copy `text` to the clipboard.
pub fn copy(text: &str) -> anyhow::Result<()> {
    if cfg!(test) {
        return Ok(());
    }
    let osc = crossterm::execute!(std::io::stdout(), crossterm::clipboard::CopyToClipboard::to_clipboard_from(text));
    let local = local_command().map(|argv| pipe(&argv, text));
    match (osc, local) {
        (_, Some(Ok(()))) | (Ok(()), _) => Ok(()),
        (Err(e), _) => Err(e.into()),
    }
}

/// The clipboard command for this machine, unless the session is remote (where it would
/// set the remote machine's clipboard).
fn local_command() -> Option<Vec<&'static str>> {
    let var = |k| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    if var("SSH_CONNECTION") || var("SSH_TTY") {
        return None;
    }
    let candidates: &[&[&str]] = if cfg!(target_os = "macos") {
        &[&["pbcopy"]]
    } else if var("WAYLAND_DISPLAY") {
        &[&["wl-copy"]]
    } else if var("DISPLAY") {
        &[&["xclip", "-selection", "clipboard"], &["xsel", "--clipboard", "--input"]]
    } else {
        &[]
    };
    candidates.iter().find(|argv| crate::app::on_path(argv[0])).map(|argv| argv.to_vec())
}

fn pipe(argv: &[&str], text: &str) -> anyhow::Result<()> {
    let mut child = Command::new(argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    child.stdin.take().expect("piped").write_all(text.as_bytes())?;
    // Some (xclip) stay running to serve the selection; reap them in the background.
    std::thread::spawn(move || child.wait());
    Ok(())
}
