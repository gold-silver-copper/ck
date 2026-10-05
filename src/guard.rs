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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_comes_back_as_its_message() {
        assert_eq!(catching(|| 2), Ok(2));
        assert_eq!(catching(|| std::panic::panic_any("static")), Err::<(), _>("static".into()));
        assert_eq!(catching(|| std::panic::panic_any(format!("formatted {}", 1))), Err::<(), _>("formatted 1".into()));
        assert_eq!(catching(|| std::panic::panic_any(7)), Err::<(), _>("a panic".into()));
    }
}
