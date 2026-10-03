//! `CK_INPUT_LOG=<path>`: a line for each input event as it's read, as it's handled, and for
//! each frame drawn, with the time since ck started. For finding out what happened to a key;
//! off, and free, unless the variable is set.

use std::io::Write;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

struct Log {
    file: std::fs::File,
    start: Instant,
}

static LOG: LazyLock<Option<Mutex<Log>>> = LazyLock::new(|| {
    let path = std::env::var_os("CK_INPUT_LOG")?;
    let file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok()?;
    Some(Mutex::new(Log { file, start: Instant::now() }))
});

/// Note something, if logging; `what` is only called then.
pub fn note(what: impl FnOnce() -> String) {
    if let Some(log) = LOG.as_ref() {
        let mut log = crate::http::lock(log);
        let at = log.start.elapsed().as_secs_f64();
        let _ = writeln!(log.file, "{at:>10.3}  {}", what());
    }
}
