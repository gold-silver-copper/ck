//! Entry points for coverage-guided fuzzing (`fuzz/`, run with `cargo fuzz`). Each takes
//! arbitrary bytes and must never panic; the targets are thin wrappers around these.

use std::sync::LazyLock;

use ratatui::style::Style;

use crate::markup::{self, Flavor};
use crate::route::{self, SiteInfo};

/// Comment HTML in every flavor (and as plain text), wrapped at a few widths, highlighted.
pub fn markup(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let width = data.first().map_or(40, |&b| b as usize);
    for flavor in [Flavor::Fourchan, Flavor::Vichan, Flavor::Lynxchan, Flavor::Jschan, Flavor::Makaba] {
        let parsed = markup::parse_html(&text, flavor);
        for line in &parsed.lines {
            for w in [0, 2, 7, width] {
                for l in markup::wrap(line, w) {
                    let _ = markup::highlight(&l, "a", Style::new());
                }
            }
            let _ = markup::reveal(line);
        }
    }
    let _ = markup::parse_plain(&text);
}

static SITES: LazyLock<Vec<SiteInfo>> = LazyLock::new(|| {
    toml::from_str::<crate::config::Config>(crate::config::DEFAULT_CONFIG)
        .map(|cfg| cfg.sites.iter().map(|s| SiteInfo::new(s, &crate::backend::build(s).board_url("x"))).collect())
        .unwrap_or_default()
});

/// What's typed after `:` or pasted.
pub fn route(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    if let Ok(t) = route::resolve(&text, &SITES, (0, Some("g"))) {
        assert!(t.site < SITES.len());
    }
    let (path, fragment) = text.split_once('#').unwrap_or((&text, ""));
    let _ = route::parse_path(path, fragment);
}

/// An API response, through every engine's parsers.
pub fn backend_json(data: &[u8]) {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(data) {
        let _ = crate::backend::parse_everything(&v);
    }
}

/// A config file, through every check `main` makes.
pub fn config(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else { return };
    let Ok(cfg) = toml::from_str::<crate::config::Config>(text) else { return };
    let _ = crate::keys::KeyMap::new(&cfg.keys);
    let _ = crate::filter::Filters::new(&cfg.filters);
    let _ = crate::theme::from_config(cfg.theme.as_ref(), &cfg.themes);
}

/// A file from the data directory, as each kind of data file.
pub fn data_file(data: &[u8]) {
    use crate::store::{Session, Visit, Watched};
    let _ = serde_json::from_slice::<Vec<Watched>>(data);
    let _ = serde_json::from_slice::<Vec<Visit>>(data);
    let _ = serde_json::from_slice::<Session>(data);
    let _ = serde_json::from_slice::<Vec<String>>(data);
    let dir = std::env::temp_dir().join(format!("ck-fuzz-data-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    for name in ["watched.json", "history.json", "hidden.json", "seen.json", "settings.json", "board_prefs.json", "recent_boards.json"] {
        let _ = std::fs::write(dir.join(name), data);
    }
    let (store, _) = crate::store::Store::load(Some(dir.clone()));
    let _ = store.save();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Image bytes, as a thumbnail or a full-size file.
pub fn image(data: &[u8]) {
    if let Ok(img) = crate::images::decode(data) {
        assert!(img.width() <= 4096 && img.height() <= 4096);
    }
    let _ = crate::images::gif_frames_within(data, 1 << 20);
}
