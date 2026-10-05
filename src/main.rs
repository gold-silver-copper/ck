use std::time::Duration;

use anyhow::Result;
use std::io::stdout;
use std::time::Instant;

use ratatui::crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui_image::picker::Picker;
use ratatui_image::picker::cap_parser::QueryStdioOptions;

use ck::app::App;
use ck::config::{self, Config, ImagesMode};
use ck::keys::KeyMap;
use ck::store::Store;
use ck::{disk_cache, filter, theme, ui};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut start_at = None;
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => {}
        ["--print-config"] => {
            print!("{}", config::fresh());
            return Ok(());
        }
        ["--print-sites"] => {
            print!("{}", config::builtin_sites_text());
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
    filter::Filters::new(&config.filters)?.with_words(&config.hidden_words)?;
    // The theme is checked now, so a bad one is reported before the terminal is taken over.
    theme::set(theme::from_config(config.theme.as_ref(), &config.themes)?);
    let (store, warnings) = Store::load(Store::dir());
    let mut terminal = ratatui::init();
    let _ = execute!(stdout(), EnableMouseCapture, EnableBracketedPaste);
    restore_on_main_thread_panics();
    ck::input_log::note(|| "images  asking the terminal".into());
    let picker = (config.images != ImagesMode::Off).then(|| detect_images(config.images));
    ck::input_log::note(|| format!("images  {:?}", picker.as_ref().map(|p| p.protocol_type())));
    let mut app = App::new(config, keys, picker, store);
    if let Some(w) = warnings.first() {
        app.error(w);
    }
    // A panic still saves what can be saved and restores the terminal (the hook has
    // already put it back and printed the message).
    let result = ck::guard::catching(|| {
        match start_at {
            Some(at) => app.goto_str(&at),
            None if app.restore_session => app.restore_session(),
            None => {}
        }
        run(&mut terminal, &mut app)
    })
    .unwrap_or_else(|what| Err(anyhow::anyhow!("ck stopped on a bug ({what}); what it had was saved")));
    app.save_session(None);
    app.flush_writes();
    app.save_now();
    let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

/// ratatui::init's panic hook restores the terminal, for a panic on any thread; one on a
/// background thread would leave ck running on a terminal no longer set up for it. Only
/// the main thread's restores (mouse capture off too); others are left to the thread's
/// owner to report, and the terminal alone.
fn restore_on_main_thread_panics() {
    let main = std::thread::current().id();
    let restore = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == main {
            let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
            restore(info);
        } else {
            ck::input_log::note(|| format!("panic   {info}"));
        }
    }));
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
       ck --print-sites    print the built-in sites (copy one into your config to change it)
       ck --help           this help

config:  {}{config_state}
data:    {}   (watched threads, history, hidden posts, tabs, board lists)
cache:   {}   (thumbnails, at most 200 MB)

Press ? inside ck for the keys. See the README for configuration.
",
        env!("CARGO_PKG_VERSION"),
        path(Config::path()),
        path(Store::dir()),
        path(disk_cache::DiskCache::default_dir()),
    )
}

/// Ask the terminal which image protocol it speaks (and its cell size); half-blocks if it
/// doesn't answer. `images` in the config may name the protocol instead. Must run after
/// entering the alternate screen and before reading any events.
fn detect_images(mode: ImagesMode) -> Picker {
    // Terminals that can't show graphics: don't wait for an answer that won't come (it
    // would also swallow the first keypress).
    if matches!(std::env::var("TERM").as_deref(), Ok("dumb" | "linux")) && mode == ImagesMode::Auto {
        return Picker::halfblocks();
    }
    const TIMEOUT: Duration = Duration::from_secs(1);
    let options = QueryStdioOptions { timeout: TIMEOUT, ..Default::default() };
    let asked = Instant::now();
    let picker = Picker::from_query_stdio_with_options(options);
    // No answer in time comes back as a guess, not an error: the time it took tells.
    if picker.is_err() || asked.elapsed() >= TIMEOUT {
        release_query_reader();
    }
    let mut picker = picker.unwrap_or_else(|_| Picker::halfblocks());
    let in_zellij = std::env::var_os("ZELLIJ").is_some();
    picker.set_protocol_type(ck::images::choose_protocol(mode, picker.protocol_type(), in_zellij));
    picker
}

/// After a terminal query that got no answer in time. ratatui-image reads the answer on a
/// thread of its own, which is still waiting on stdin, and would take the first key typed
/// (tmux, unless `allow-passthrough` is on, drops the whole query, so nothing ever comes).
/// Ask what every terminal answers, a status report, so that thread gets its answer and
/// ends before ck starts reading input. If the reply comes to ck instead, it's dropped as
/// an unknown sequence.
fn release_query_reader() {
    use std::io::Write;
    let mut out = stdout();
    if out.write_all(b"\x1b[5n").and_then(|()| out.flush()).is_ok() {
        std::thread::sleep(Duration::from_millis(100));
    }
    ck::input_log::note(|| "images  no answer: asked for a status report to end the query".into());
}

/// Draw, then sleep until input, a finished request, or the next deadline.
fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    app.listen_for_input();
    app.poll();
    // Debugging: each frame as ck means it to look, to compare with what's on screen.
    let dump = std::env::var_os("CK_FRAME_DUMP").map(std::path::PathBuf::from);
    let mut screen = None;
    while !app.quit {
        // Another screen: paint it whole, so nothing of the last one can stay behind (an
        // image, or text a terminal placed differently than measured). Not with `clear()`:
        // that asks the terminal where the cursor is, and the answer would go to the input
        // thread instead (and ck would stop with "the cursor position could not be read").
        let now = Some(app.screen());
        if screen != now {
            if screen.is_some() {
                let size = terminal.size()?;
                terminal.resize(ratatui::layout::Rect::new(0, 0, size.width, size.height))?;
            }
            screen = now;
        }
        let frame = terminal.draw(|f| ui::draw(f, app))?;
        ck::input_log::note(|| "frame".into());
        if let Some(path) = &dump {
            let _ = std::fs::write(path, frame_text(frame.buffer));
        }
        let timeout = app.next_wake(Instant::now());
        ck::input_log::note(|| format!("sleep   up to {timeout:?}"));
        if !app.wait(timeout) {
            app.tick = app.tick.wrapping_add(1);
        }
    }
    Ok(())
}

/// A frame's text, row by row (a wide character once, as a terminal shows it, measured as
/// ratatui places it).
fn frame_text(buf: &ratatui::buffer::Buffer) -> String {
    use ratatui::buffer::CellWidth;
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut row = String::new();
        let mut x = 0;
        while x < buf.area.width {
            let symbol = buf.cell((x, y)).map_or(" ", |c| c.symbol());
            row.push_str(symbol);
            x += symbol.cell_width().max(1);
        }
        out.push_str(row.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn only_a_main_thread_panic_restores_the_terminal() {
        let restored = Arc::new(AtomicUsize::new(0));
        let count = restored.clone();
        std::panic::set_hook(Box::new(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
        }));
        super::restore_on_main_thread_panics();
        assert!(std::thread::spawn(|| std::panic::panic_any("in the background")).join().is_err());
        assert_eq!(restored.load(Ordering::SeqCst), 0);
        assert!(std::panic::catch_unwind(|| std::panic::panic_any("on the main thread")).is_err());
        assert_eq!(restored.load(Ordering::SeqCst), 1);
        let _ = std::panic::take_hook();
    }
}
