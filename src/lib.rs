//! ck: a read-only terminal imageboard browser. The binary (`main.rs`) is a thin shell
//! around this library, which is also what the fuzz targets in `fuzz/` link against.

/// `as_str` (the name the config and the screen use) and `next` (cycling in this order)
/// for an enum of options.
macro_rules! cycle {
    ($t:ident { $($v:ident => $s:literal),+ $(,)? }) => {
        impl $t {
            pub fn as_str(self) -> &'static str {
                match self {
                    $($t::$v => $s,)+
                }
            }

            pub fn next(self) -> Self {
                const ALL: &[$t] = &[$($t::$v),+];
                let i = ALL.iter().position(|&v| v == self).unwrap_or(0);
                ALL[(i + 1) % ALL.len()]
            }
        }
    };
}

/// Whether ck may reach outside itself: open a browser or player, write the clipboard, send
/// notifications. Not in tests, nor with `CK_NO_EXTERNAL` set (the end-to-end fuzzer).
pub fn sandboxed() -> bool {
    cfg!(test) || std::env::var_os("CK_NO_EXTERNAL").is_some()
}

pub mod app;
pub mod backend;
pub mod clipboard;
#[cfg(test)]
mod bench;
pub mod config;
pub mod disk_cache;
pub mod download;
#[cfg(test)]
mod e2e;
pub mod export;
pub mod filter;
#[doc(hidden)]
pub mod fuzzing;
#[cfg(test)]
mod fuzz;
pub mod http;
pub mod images;
pub mod input_log;
pub mod keys;
pub mod markup;
pub mod model;
pub mod pages;
pub mod notify;
pub mod route;
pub mod saved;
pub mod saved_search;
pub mod store;
#[cfg(test)]
mod test_fixtures;
pub mod theme;
pub mod ui;
pub mod writer;
#[cfg(test)]
mod ui_tests;
