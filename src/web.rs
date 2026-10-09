//! ck-web, the browser helper ck posts to 4chan through (see `ck-web/`): finding it,
//! starting it, and talking to it.

pub mod install;
mod protocol;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

use anyhow::{Context, Result};
pub use protocol::{Reply, Request};

const NAME: &str = "ck-web";

/// A running ck-web. It stops when this is dropped (its stdin closes, and it's killed).
pub struct Helper {
    child: Child,
    stdin: ChildStdin,
}

impl Helper {
    /// Start the helper at `path`; `on_reply` gets each of its replies, on a thread of its
    /// own, until it returns false or the helper ends: then with None, after the last thing it
    /// printed (a missing library, a crash), as a failure.
    pub fn start(path: &Path, on_reply: impl Fn(Option<Reply>) -> bool + Send + 'static) -> Result<Helper> {
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Couldn't start {}", path.display()))?;
        let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            anyhow::bail!("Couldn't talk to {}", path.display());
        };
        let last_words = std::thread::spawn(move || BufReader::new(stderr).lines().map_while(Result::ok).filter(|l| !l.trim().is_empty()).last());
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                // A line that isn't a reply (from a newer or older ck-web) is skipped.
                if let Ok(reply) = serde_json::from_str(&line)
                    && !on_reply(Some(reply))
                {
                    return;
                }
            }
            if let Ok(Some(said)) = last_words.join() {
                on_reply(Some(Reply::Failed { error: format!("ck-web stopped: {said}") }));
            }
            on_reply(None);
        });
        Ok(Helper { child, stdin })
    }

    pub fn send(&mut self, request: &Request) -> Result<()> {
        let line = serde_json::to_string(request)?;
        writeln!(self.stdin, "{line}").and_then(|()| self.stdin.flush()).context("ck-web has stopped")
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Where ck-web is: the configured path, else as downloaded, else next to ck, else on the
/// PATH. Tests find none.
pub fn find(configured: Option<&str>) -> Option<PathBuf> {
    if crate::sandboxed() {
        return None;
    }
    if let Some(p) = configured {
        return Some(crate::config::expand_home(p));
    }
    if let Some(p) = install::installed() {
        return Some(p);
    }
    let exe = format!("{NAME}{}", std::env::consts::EXE_SUFFIX);
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(&exe)));
    let on_path = std::env::var_os("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).map(|d| d.join(&exe));
    beside.into_iter().chain(on_path).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // page.js writes its replies by hand.
    fn replies_read_as_page_js_writes_them() {
        let posted: Reply = serde_json::from_str(r#"{"is":"posted","thread":1,"no":2}"#).unwrap();
        assert_eq!(posted, Reply::Posted { thread: 1, no: 2 });
    }
}
