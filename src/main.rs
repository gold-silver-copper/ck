mod app;
mod backend;
#[cfg(test)]
mod bench;
mod config;
mod disk_cache;
mod download;
mod http;
mod images;
mod keys;
mod markup;
mod model;
mod store;
mod theme;
mod ui;
#[cfg(test)]
mod ui_tests;

use std::time::Duration;

use anyhow::Result;
use std::io::stdout;
use std::time::Instant;

use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui_image::picker::Picker;
use ratatui_image::picker::cap_parser::QueryStdioOptions;

use crate::app::App;
use crate::config::{Config, ImagesMode};
use crate::keys::KeyMap;
use crate::theme::Theme;
use crate::store::Store;

fn main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("--print-config") => {
            print!("{}", config::DEFAULT_CONFIG);
            return Ok(());
        }
        Some("-h" | "--help") => {
            print!("{}", help_text());
            return Ok(());
        }
        _ => {}
    }

    let config = Config::load()?;
    // Config errors are reported before the terminal is taken over.
    let keys = KeyMap::new(&config.keys)?;
    theme::init(Theme::from_config(&config.theme)?);
    let (store, warnings) = Store::load(Store::dir());
    let mut terminal = ratatui::init();
    // ratatui::init restores the terminal on panic; also turn mouse capture off first.
    let _ = execute!(stdout(), EnableMouseCapture);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        hook(info);
    }));
    let picker = (config.images == ImagesMode::Auto).then(detect_images);
    let mut app = App::new(config, keys, picker, store);
    if let Some(w) = warnings.first() {
        app.status = Some((w.clone(), true));
    }
    let result = run(&mut terminal, &mut app);
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

fn help_text() -> String {
    let path = |p: Option<std::path::PathBuf>| p.map_or("(no home directory)".into(), |p| p.display().to_string());
    let config_state = if Config::path().is_some_and(|p| p.exists()) { "" } else { " (not created; using defaults)" };
    format!(
        "ck {} - browse imageboards from the terminal (read-only)

usage: ck                  start
       ck --print-config   print the default config (a starting point for your own)
       ck --help           this help

config:  {}{config_state}
data:    {}   (watched threads, history, saved board lists)
cache:   {}   (thumbnails, at most 200 MB)

Press ? inside ck for the keys. See the README for configuration.
",
        env!("CARGO_PKG_VERSION"),
        path(Config::path()),
        path(Store::dir()),
        path(disk_cache::DiskCache::default_dir()),
    )
}

/// Ask the terminal which image protocol it speaks; half-blocks if it doesn't answer.
/// Must run after entering the alternate screen and before reading any events.
fn detect_images() -> Picker {
    // Terminals that can't show graphics: don't wait for an answer that won't come (it
    // would also swallow the first keypress).
    if matches!(std::env::var("TERM").as_deref(), Ok("dumb" | "linux")) {
        return Picker::halfblocks();
    }
    let options = QueryStdioOptions { timeout: Duration::from_secs(1), ..Default::default() };
    Picker::from_query_stdio_with_options(options).unwrap_or_else(|_| Picker::halfblocks())
}

/// Draw, then sleep until input, a finished request, or the next deadline.
fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    app.listen_for_input();
    app.poll();
    while !app.quit {
        terminal.draw(|f| ui::draw(f, app))?;
        let timeout = app.next_wake(Instant::now());
        if !app.wait(timeout) {
            app.tick = app.tick.wrapping_add(1);
        }
    }
    Ok(())
}
