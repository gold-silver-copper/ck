mod app;
mod backend;
mod clipboard;
#[cfg(test)]
mod bench;
mod config;
mod disk_cache;
mod download;
mod export;
mod filter;
mod http;
mod images;
mod keys;
mod markup;
mod model;
mod notify;
mod route;
mod store;
mod theme;
mod ui;
#[cfg(test)]
mod ui_tests;

use std::time::Duration;

use anyhow::Result;
use std::io::stdout;
use std::time::Instant;

use ratatui::crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui_image::picker::Picker;
use ratatui_image::picker::cap_parser::QueryStdioOptions;

use crate::app::App;
use crate::config::{Config, ImagesMode};
use crate::keys::KeyMap;
use crate::store::Store;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut start_at = None;
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => {}
        ["--print-config"] => {
            print!("{}", config::DEFAULT_CONFIG);
            return Ok(());
        }
        ["-h" | "--help"] => {
            print!("{}", help_text());
            return Ok(());
        }
        [a] if !a.starts_with('-') => start_at = Some(a.to_string()),
        _ => anyhow::bail!("usage: ck [URL | site/board/thread]   (ck --help for more)"),
    }

    let config = Config::load()?;
    // Config errors are reported before the terminal is taken over.
    let keys = KeyMap::new(&config.keys)?;
    filter::Filters::new(&config.filters)?;
    // The theme is checked now, so a bad one is reported before the terminal is taken over.
    theme::set(theme::from_config(config.theme.as_ref(), &config.themes)?);
    let (store, warnings) = Store::load(Store::dir());
    let mut terminal = ratatui::init();
    // ratatui::init restores the terminal on panic; also turn mouse capture off first.
    let _ = execute!(stdout(), EnableMouseCapture, EnableBracketedPaste);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
        hook(info);
    }));
    let picker = (config.images == ImagesMode::Auto).then(detect_images);
    let mut app = App::new(config, keys, picker, store);
    if let Some(w) = warnings.first() {
        app.status = Some((w.clone(), true));
    }
    if let Some(at) = start_at {
        app.goto_str(&at);
    }
    let result = run(&mut terminal, &mut app);
    let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

fn help_text() -> String {
    let path = |p: Option<std::path::PathBuf>| p.map_or("(no home directory)".into(), |p| p.display().to_string());
    let config_state = if Config::path().is_some_and(|p| p.exists()) { "" } else { " (not created; using defaults)" };
    format!(
        "ck {} - browse imageboards from the terminal (read-only)

usage: ck                  start
       ck URL              start at a board or thread: a URL, or a short form like
                           4chan/g, 4chan/g/123 or lainchan/λ/42#43
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
