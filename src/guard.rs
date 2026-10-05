//! Panics kept from doing more harm than the bug that caused them: the main loop's ends in
//! a saved session and a restored terminal, a background job's in an error its owner shows.

use std::panic::{AssertUnwindSafe, catch_unwind};

/// Run `f`; a panic in it comes back as its message.
pub fn catching<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|panic| {
        let what = panic.downcast_ref::<&str>().copied().or_else(|| panic.downcast_ref::<String>().map(String::as_str));
        what.unwrap_or("a panic").to_string()
    })
}

/// Run a job whose failure its owner reports; a panic in it is one more failure.
pub fn result<T>(f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    catching(f).unwrap_or_else(|what| Err(anyhow::anyhow!("ck hit a bug: {what}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_comes_back_as_its_message() {
        assert_eq!(catching(|| 2), Ok(2));
        assert_eq!(catching(|| std::panic::panic_any("static")), Err::<(), _>("static".into()));
        assert_eq!(catching(|| std::panic::panic_any(format!("formatted {}", 1))), Err::<(), _>("formatted 1".into()));
        assert_eq!(catching(|| std::panic::panic_any(7)), Err::<(), _>("a panic".into()));
        let failed = result::<()>(|| std::panic::panic_any("index out of bounds")).unwrap_err();
        assert_eq!(failed.to_string(), "ck hit a bug: index out of bounds");
    }
}
