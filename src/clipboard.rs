//! Reading files and images with local clipboard commands. Copying text uses OSC 52
//! (works over SSH, and in tmux with `set-clipboard on`) plus a local clipboard command.

use anyhow::Context;
use std::io::{Read, Write};
use std::process::{Command, Stdio};

/// Copy `text` to the clipboard.
pub fn copy(text: &str) -> anyhow::Result<()> {
    if crate::sandboxed() {
        return Ok(());
    }
    let osc = crossterm::execute!(std::io::stdout(), crossterm::clipboard::CopyToClipboard::to_clipboard_from(text));
    let local = local_command(false).map(|argv| pipe(&argv, text));
    match (osc, local) {
        (_, Some(Ok(()))) | (Ok(()), _) => Ok(()),
        (Err(e), _) => Err(e.into()),
    }
}

/// The clipboard command for this machine, unless the session is remote (where it would
/// access the remote machine's clipboard).
fn local_command(read: bool) -> Option<Vec<&'static str>> {
    let var = |k| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    if var("SSH_CONNECTION") || var("SSH_TTY") {
        return None;
    }
    let candidates: &[&[&str]] = if cfg!(target_os = "macos") {
        if read { &[&["osascript"]] } else { &[&["pbcopy"]] }
    } else if var("WAYLAND_DISPLAY") {
        if read { &[&["wl-paste"]] } else { &[&["wl-copy"]] }
    } else if var("DISPLAY") {
        if read { &[&["xclip", "-selection", "clipboard", "-o"]] } else { &[&["xclip", "-selection", "clipboard"], &["xsel", "--clipboard", "--input"]] }
    } else {
        &[]
    };
    candidates.iter().find(|argv| argv.first().is_some_and(|cmd| crate::app::on_path(cmd))).map(|argv| argv.to_vec())
}

fn pipe(argv: &[&str], text: &str) -> anyhow::Result<()> {
    let Some((cmd, args)) = argv.split_first() else { anyhow::bail!("no clipboard command") };
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    // Some (xclip) stay running to serve the selection; reap them in the background.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// A pasted path, including a file manager's URL or a shell-quoted drag.
pub fn file_path(text: &str) -> String {
    let text = text.trim().lines().next().unwrap_or_default().trim();
    // Terminals quote each path in a multi-file drag, or escape its spaces.
    let text = if let Some(quote @ ('\'' | '"')) = text.chars().next() {
        text.get(1..).and_then(|s| s.find(quote).and_then(|end| s.get(..end))).unwrap_or(text)
    } else if text.contains("\\ ") {
        let mut escaped = false;
        let end = text.char_indices().find_map(|(i, ch)| {
            if escaped { escaped = false; return None; }
            if ch == '\\' { escaped = true; return None; }
            ch.is_whitespace().then_some(i)
        }).unwrap_or(text.len());
        text.get(..end).unwrap_or(text)
    } else {
        text.split(" /").next().unwrap_or(text).split(" file://").next().unwrap_or(text)
    };
    if let Some(url) = text.strip_prefix("file://") {
        let path = url.strip_prefix("localhost/").map_or_else(|| url.to_string(), |s| format!("/{s}"));
        let mut bytes = path.bytes();
        let mut decoded = Vec::new();
        while let Some(b) = bytes.next() {
            if b == b'%' {
                let mut peek = bytes.clone();
                if let Some(n) = peek.next().and_then(hex).zip(peek.next().and_then(hex)).map(|(a, b)| a * 16 + b) {
                    decoded.push(n);
                    bytes = peek;
                    continue;
                }
            }
            decoded.push(b);
        }
        return String::from_utf8(decoded).unwrap_or(path);
    }
    text.replace("\\ ", " ")
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|n| n as u8)
}

/// Read on a worker: clipboard owners can take time to supply their data.
pub fn read_file() -> anyhow::Result<String> {
    anyhow::ensure!(!crate::sandboxed(), "Clipboard reading is disabled in the sandbox");
    let argv = local_command(true).context("No local clipboard reader (unavailable over SSH; install wl-clipboard or xclip on Linux)")?;
    if cfg!(target_os = "macos") {
        if let Ok(data) = read_command(&argv, &["-e", "get the clipboard as «class furl» as text"]) {
            let path = file_path(std::str::from_utf8(&data)?);
            if !path.is_empty() {
                return Ok(path);
            }
        }
        let data = read_command(&argv, &["-e", "get the clipboard as «class PNGf»"])?;
        let data = std::str::from_utf8(&data)?.trim().strip_prefix("«data PNGf")
            .and_then(|s| s.strip_suffix('»')).context("Clipboard has no file or PNG image")?;
        let png: Option<Vec<u8>> = data.as_bytes().chunks(2).map(|pair| {
            let mut pair = pair.iter();
            Some(hex(*pair.next()?)? * 16 + hex(*pair.next()?)?)
        }).collect();
        return save_image(&png.context("Invalid clipboard PNG data")?, "png");
    }
    let wayland = argv.first() == Some(&"wl-paste");
    let list = if wayland { vec!["--list-types"] } else { vec!["-t", "TARGETS"] };
    let types = read_command(&argv, &list)?;
    let types = std::str::from_utf8(&types)?;
    let kind = ["text/uri-list", "image/png", "image/jpeg", "image/webp", "text/plain;charset=utf-8", "UTF8_STRING", "text/plain"]
        .into_iter().find(|kind| types.lines().any(|line| line.trim() == *kind))
        .context("Clipboard has no file, path or PNG/JPEG/WebP image")?;
    let args = if wayland { vec!["--no-newline", "--type", kind] } else { vec!["-t", kind] };
    let data = read_command(&argv, &args)?;
    if let Some(format) = kind.strip_prefix("image/") {
        return save_image(&data, if format == "jpeg" { "jpg" } else { format });
    }
    let text = std::str::from_utf8(&data)?;
    let text = if kind == "text/uri-list" {
        text.lines().find(|s| !s.trim().is_empty() && !s.starts_with('#')).context("Clipboard file list is empty")?
    } else { text };
    let path = file_path(text);
    anyhow::ensure!(!path.is_empty(), "Clipboard path is empty");
    Ok(path)
}

fn save_image(data: &[u8], ext: &str) -> anyhow::Result<String> {
    anyhow::ensure!(!data.is_empty(), "Clipboard image is empty");
    let dir = crate::config::folder(crate::config::Folder::Cache).context("No cache folder")?;
    let path = dir.join("pasted").join(format!("clipboard-{:016x}.{ext}", crate::disk_cache::fnv1a128(data) as u64));
    crate::atomic::write_from(&path, data)?;
    Ok(path.to_string_lossy().into_owned())
}

fn read_command(argv: &[&str], args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let (cmd, base) = argv.split_first().context("No clipboard command")?;
    let mut child = Command::new(cmd).args(base).args(args).stdin(Stdio::null())
        .stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let stdout = child.stdout.take().context("No clipboard output")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut data = Vec::new();
        let result = stdout.take(64 * 1024 * 1024 + 1).read_to_end(&mut data).map(|_| data);
        let _ = tx.send(result);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let result = (|| {
        loop {
            if let Some(status) = child.try_wait()? {
                anyhow::ensure!(status.success(), "Clipboard command failed");
                let data = rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))??;
                anyhow::ensure!(data.len() <= 64 * 1024 * 1024, "Clipboard image is too large");
                return Ok(data);
            }
            anyhow::ensure!(std::time::Instant::now() < deadline, "Clipboard read timed out");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_paths() {
        for (text, path) in [
            ("  '/home/me/a b.png'\n", "/home/me/a b.png"),
            ("\"/home/me/a b.png\"", "/home/me/a b.png"),
            ("/home/me/a\\ b.png", "/home/me/a b.png"),
            ("file:///home/me/a%20b%23%C3%A9.png", "/home/me/a b#é.png"),
            ("file://localhost/home/me/a.png", "/home/me/a.png"),
            ("file:///a%xx.png", "/a%xx.png"),
            ("'/a b.png' '/c d.png'", "/a b.png"),
            ("/a\\ b.png /c\\ d.png", "/a b.png"),
            ("/a.png /b.png", "/a.png"),
            ("file:///a.png\r\nfile:///b.png", "/a.png"),
            ("/home/me/a b.png", "/home/me/a b.png"),
            ("/home/me/100%20.png", "/home/me/100%20.png"),
        ] {
            assert_eq!(file_path(text), path);
        }
    }
}
