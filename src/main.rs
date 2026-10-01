mod app;
mod backend;
mod config;
mod http;
mod images;
mod markup;
mod model;
mod store;
mod ui;

use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui_image::picker::Picker;
use ratatui_image::picker::cap_parser::QueryStdioOptions;

use crate::app::App;
use crate::config::{Config, ImagesMode};
use crate::store::Store;

fn main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("--print-config") => {
            print!("{}", config::DEFAULT_CONFIG);
            return Ok(());
        }
        Some("-h" | "--help") => {
            println!("ck - browse imageboards from the terminal\n");
            println!("usage: ck [--print-config]\n");
            match Config::path() {
                Some(p) => println!("config: {}", p.display()),
                None => println!("config: (no home directory)"),
            }
            return Ok(());
        }
        _ => {}
    }

    let config = Config::load()?;
    let (store, warnings) = Store::load(Store::dir());
    let mut terminal = ratatui::init();
    let picker = (config.images == ImagesMode::Auto).then(detect_images);
    let mut app = App::new(config, picker, store);
    if let Some(w) = warnings.first() {
        app.status = Some((w.clone(), true));
    }
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result
}

/// Ask the terminal which image protocol it speaks; half-blocks if it doesn't answer.
/// Must run after entering the alternate screen and before reading any events.
fn detect_images() -> Picker {
    let options = QueryStdioOptions { timeout: Duration::from_secs(1), ..Default::default() };
    Picker::from_query_stdio_with_options(options).unwrap_or_else(|_| Picker::halfblocks())
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    while !app.quit {
        app.poll();
        terminal.draw(|f| ui::draw(f, app))?;
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                _ => {}
            }
        } else {
            app.tick = app.tick.wrapping_add(1);
        }
    }
    Ok(())
}
