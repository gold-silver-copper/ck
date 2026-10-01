mod app;
mod backend;
mod config;
mod http;
mod markup;
mod model;
mod ui;

use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use crate::app::App;
use crate::config::Config;

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
    let mut app = App::new(config);
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result
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
