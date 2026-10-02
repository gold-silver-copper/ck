//! Desktop notifications through the terminal (OSC 9 / OSC 777), the bell, or a command.

use std::io::Write;
use std::process::{Command, Stdio};

use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyMode {
    /// A desktop notification where the terminal is known to show them, else the bell.
    #[default]
    Auto,
    Bell,
    Off,
}

cycle!(NotifyMode { Auto => "auto", Bell => "bell", Off => "off" });

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    /// `ESC ] 9 ; body BEL`: iTerm2, kitty, WezTerm, Ghostty, Windows Terminal.
    Osc9,
    /// `ESC ] 777 ; notify ; title ; body BEL`: foot, urxvt.
    Osc777,
    Bell,
    /// A program, with `{title}` and `{body}` filled in (no shell).
    Command(Vec<String>),
    Off,
}

/// How to notify, from the config and the environment.
pub fn method(mode: NotifyMode, command: Option<&[String]>, env: &dyn Fn(&str) -> Option<String>) -> Method {
    match (mode, command) {
        (NotifyMode::Off, _) => Method::Off,
        (_, Some(argv)) if !argv.is_empty() => Method::Command(argv.to_vec()),
        (NotifyMode::Bell, _) => Method::Bell,
        (NotifyMode::Auto, _) => {
            let has = |k: &str| env(k).is_some_and(|v| !v.is_empty());
            let term = env("TERM").unwrap_or_default();
            let program = env("TERM_PROGRAM").unwrap_or_default();
            // Inside tmux or screen the escape would need passthrough; the bell gets through.
            if has("TMUX") || term.starts_with("screen") || term.starts_with("tmux") {
                Method::Bell
            } else if matches!(program.as_str(), "iTerm.app" | "WezTerm" | "ghostty")
                || has("KITTY_WINDOW_ID")
                || has("WT_SESSION")
                || term == "xterm-kitty"
                || term == "xterm-ghostty"
            {
                Method::Osc9
            } else if term.starts_with("foot") || term.starts_with("rxvt-unicode") {
                Method::Osc777
            } else {
                Method::Bell
            }
        }
    }
}

/// Send a notification. Escapes go straight to the terminal.
pub fn send(method: &Method, title: &str, body: &str) -> anyhow::Result<()> {
    if crate::sandboxed() {
        return Ok(());
    }
    // Control characters (and 777's separator) can't be in the escape.
    let clean = |s: &str| s.chars().map(|c| if c.is_control() || c == ';' { ' ' } else { c }).collect::<String>();
    let out = match method {
        Method::Off => return Ok(()),
        Method::Osc9 => format!("\x1b]9;{}\x07", clean(body)),
        Method::Osc777 => format!("\x1b]777;notify;{};{}\x07", clean(title), clean(body)),
        Method::Bell => "\x07".to_string(),
        Method::Command(argv) => {
            let fill = |a: &String| a.replace("{title}", title).replace("{body}", body);
            let mut child = Command::new(fill(&argv[0]))
                .args(argv[1..].iter().map(fill))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            std::thread::spawn(move || child.wait());
            return Ok(());
        }
    };
    let mut stdout = std::io::stdout();
    stdout.write_all(out.as_bytes())?;
    Ok(stdout.flush()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: Vec<(String, String)> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    }

    #[test]
    fn picks_a_method_for_the_terminal() {
        let auto = |vars: &[(&str, &str)]| method(NotifyMode::Auto, None, &with(vars));
        assert_eq!(auto(&[("TERM_PROGRAM", "iTerm.app")]), Method::Osc9);
        assert_eq!(auto(&[("KITTY_WINDOW_ID", "1"), ("TERM", "xterm-kitty")]), Method::Osc9);
        assert_eq!(auto(&[("TERM", "foot")]), Method::Osc777);
        assert_eq!(auto(&[("TERM", "xterm-256color")]), Method::Bell);
        // Inside tmux, whatever the outer terminal: the bell.
        assert_eq!(auto(&[("TERM_PROGRAM", "iTerm.app"), ("TMUX", "/tmp/x,1,0")]), Method::Bell);
        let cmd = vec!["notify-send".to_string(), "{title}".into(), "{body}".into()];
        assert_eq!(method(NotifyMode::Auto, Some(&cmd), &with(&[])), Method::Command(cmd.clone()));
        assert_eq!(method(NotifyMode::Off, Some(&cmd), &with(&[])), Method::Off);
        assert_eq!(method(NotifyMode::Bell, None, &with(&[("TERM", "foot")])), Method::Bell);
    }
}
