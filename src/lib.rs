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
                // The one after, or round to the first.
                ALL.get(i + 1).or(ALL.first()).copied().unwrap_or(self)
            }
        }
    };
}

/// Whether ck may reach outside itself: open a browser or player, write the clipboard, send
/// notifications. Not in tests, nor with `CK_NO_EXTERNAL` set (the end-to-end fuzzer).
pub fn sandboxed() -> bool {
    cfg!(test) || std::env::var_os("CK_NO_EXTERNAL").is_some()
}

pub(crate) mod app;
pub(crate) mod backend;
pub(crate) mod clipboard;
#[cfg(test)]
mod bench;
pub(crate) mod config;
pub(crate) mod disk_cache;
pub(crate) mod download;
#[cfg(test)]
mod e2e;
pub(crate) mod export;
pub(crate) mod filter;
#[doc(hidden)]
pub mod fuzzing;
#[cfg(test)]
mod fuzz;
pub(crate) mod guard;
pub(crate) mod http;
pub(crate) mod images;
pub mod input_log;
pub(crate) mod keys;
pub(crate) mod markup;
pub(crate) mod model;
pub(crate) mod pages;
pub(crate) mod notify;
pub(crate) mod route;
pub(crate) mod saved;
pub(crate) mod saved_search;
pub(crate) mod store;
#[cfg(test)]
mod test_fixtures;
pub(crate) mod theme;
pub(crate) mod title;
pub(crate) mod ui;
pub(crate) mod writer;
#[cfg(test)]
mod ui_tests;

// What the binary (`main.rs`) uses. Everything else is the crate's own, so the compiler
// can tell what nothing uses.
pub use app::{App, start_error};
pub use config::{Config, ImagesMode, builtin_sites_text, fresh as fresh_config};
pub use disk_cache::DiskCache;
pub use download::default_root as download_root;
pub use filter::Filters;
pub use guard::catching;
pub use images::choose_protocol;
pub use keys::KeyMap;
pub use pages::Pages;
pub use store::Store;
pub use theme::{from_config as theme_from_config, set as set_theme};
pub use ui::draw;
