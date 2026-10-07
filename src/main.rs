use std::time::Duration;

use anyhow::Result;
use std::io::stdout;
use std::time::Instant;

use ratatui::crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
use ratatui::crossterm::terminal::EnterAlternateScreen;
use ratatui::crossterm::execute;
use ratatui_image::picker::Picker;
use ratatui_image::picker::cap_parser::QueryStdioOptions;

use ck::{App, Config, DiskCache, Filters, ImagesMode, KeyMap, Pages, Store};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut start_at = None;
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => {}
        ["--print-config"] => {
            say(&ck::fresh_config().to_string());
            return Ok(());
        }
        ["--print-sites"] => {
            say(&ck::builtin_sites_text());
            return Ok(());
        }
        ["-h" | "--help"] => {
            say(&help_text());
            return Ok(());
        }
        ["-V" | "--version"] => {
            say(&format!("ck {}\n", env!("CARGO_PKG_VERSION")));
            return Ok(());
        }
        [a] if !a.starts_with('-') => start_at = Some(a.to_string()),
        [a] => anyhow::bail!("ck doesn't know the option {a}\n{USAGE}"),
        many => anyhow::bail!("ck takes one place to start at, not {}: {}\n{USAGE}", many.len(), many.join(" ")),
    }

    let config = Config::load()?;
    if let Some(e) = start_at.as_deref().and_then(|at| ck::start_error(&config.sites, at)) {
        anyhow::bail!("can't start at {}: {e}", start_at.unwrap_or_default());
    }
    // Config errors are reported before the terminal is taken over.
    let keys = KeyMap::new(&config.keys)?;
    let filters = Filters::from_config(&config.filters, &config.hidden_words)?;
    // The theme is checked now, so a bad one is reported before the terminal is taken over.
    ck::set_theme(ck::theme_from_config(config.theme.as_ref(), &config.themes)?);
    let (store, warnings) = Store::load(Store::dir());
    let mut terminal = take_terminal().inspect_err(|_| give_terminal_back())?;
    restore_on_main_thread_panics();
    ck::input_log::note(|| "images  asking the terminal".into());
    let detected = (config.images != ImagesMode::Off).then(|| detect_images(config.images));
    let (picker, images_note) = detected.map_or((None, None), |(p, note)| (Some(p), note));
    ck::input_log::note(|| format!("images  {:?}", picker.as_ref().map(|p| p.protocol_type())));
    let mut app = App::new(config, keys, filters, picker, store);
    if let Some(note) = images_note {
        app.info(note);
    }
    #[cfg(unix)]
    if let Err(e) = app.quit_on_signals() {
        app.error(format!("Closing the terminal won't save first: {e}"));
    }
    if let Some(w) = warnings.first() {
        app.error(w);
    }
    // A panic still saves what can be saved and restores the terminal (the hook has
    // already put it back and printed the message).
    let result = ck::catching(|| {
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
    // (A panic or a signal ends up here too.)
    app.restore_title();
    give_terminal_back();
    // Not dropped: when it can't show the cursor (the terminal gone), its drop eprintln!s
    // and panics.
    let _ = terminal.show_cursor();
    std::mem::forget(terminal);
    match app.quit_because {
        Some(why) => result.and(Err(anyhow::anyhow!("{why}"))),
        None => result,
    }
}

/// The terminal set up for ck: raw mode, the alternate screen, mouse and paste. Not with
/// ratatui::init: its panic hook restores with ratatui::restore, which panics when it can't
/// write its error to stderr (the terminal gone: the window closed), and a panic inside a
/// panic hook aborts.
fn take_terminal() -> std::io::Result<ratatui::DefaultTerminal> {
    ratatui::crossterm::terminal::enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let _ = execute!(stdout(), EnableMouseCapture, EnableBracketedPaste);
    ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout()))
}

/// Put the terminal back as it was. Never fails or panics: by now it may be gone.
fn give_terminal_back() {
    let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
    let _ = ratatui::try_restore();
}

/// A panic on the main thread puts the terminal back, then reports as usual; one on a
/// background thread is left to the thread's owner to report, and the terminal alone (ck
/// goes on running on it).
fn restore_on_main_thread_panics() {
    let main = std::thread::current().id();
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == main {
            give_terminal_back();
            report(info);
        } else {
            ck::input_log::note(|| format!("panic   {info}"));
        }
    }));
}

/// Print to stdout, which may be a closed pipe (`ck --help | head -1`): print! would panic.
fn say(text: &str) {
    use std::io::Write;
    let mut out = stdout();
    let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
}

const USAGE: &str = "usage: ck [URL | site/board/thread]   (ck --help for more)";

fn help_text() -> String {
    let path = |p: Option<std::path::PathBuf>| p.map_or_else(|| "(no home directory)".into(), |p| p.display().to_string());
    let config_state = if Config::path().is_some_and(|p| p.exists()) { "" } else { " (not created; using defaults)" };
    format!(
        "ck {} - browse imageboards from the terminal (read-only)

usage: ck                  start
       ck URL              start at a board or thread: a URL, or a short form like
                           4chan/g, 4chan/g/123 or lainchan/λ/42#43
       ck --print-config   print the default config (a starting point for your own)
       ck --print-sites    print the built-in sites (copy one into your config to change it)
       ck --version        the version
       ck --help           this help

config:  {}{config_state}
data:    {}   (watched threads, history, hidden posts, tabs, board lists)
cache:   {}   (thumbnails, at most 200 MB)
pages:   {}   (the last copy of each catalog and thread, page_cache_mb)
files:   {}   (downloads, in a folder per thread; download_dir)

Press ? inside ck for the keys. The manual (docs/manual.md) has the rest.
",
        env!("CARGO_PKG_VERSION"),
        path(Config::path()),
        path(Store::dir()),
        path(DiskCache::default_dir()),
        path(Pages::default_dir()),
        ck::download_root().display(),
    )
}

/// Ask the terminal which image protocol it speaks (and its cell size); half-blocks if it
/// doesn't answer. `images` in the config may name the protocol instead. Must run after
/// entering the alternate screen and before reading any events. Also something to tell the
/// user, when there's a setting that would make images sharper.
fn detect_images(mode: ImagesMode) -> (Picker, Option<String>) {
    // Terminals that can't show graphics: don't wait for an answer that won't come (it
    // would also swallow the first keypress).
    if matches!(std::env::var("TERM").as_deref(), Ok("dumb" | "linux")) && mode == ImagesMode::Auto {
        return (Picker::halfblocks(), None);
    }
    // tmux without passthrough drops the question: waiting a second for nothing every start.
    if mode == ImagesMode::Auto && std::env::var_os("TMUX").is_some() && tmux_passthrough() == Some(false) {
        ck::input_log::note(|| "images  tmux without allow-passthrough: half-blocks, not asking".into());
        return (Picker::halfblocks(), Some("Images are half-blocks: in tmux, `set -g allow-passthrough on` lets the terminal draw them sharp".into()));
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
    picker.set_protocol_type(ck::choose_protocol(mode, picker.protocol_type(), in_zellij));
    (picker, None)
}

/// Whether the tmux ck runs in passes escapes through to the terminal (`None`: couldn't
/// tell).
fn tmux_passthrough() -> Option<bool> {
    let out = std::process::Command::new("tmux").args(["show", "-gv", "allow-passthrough"]).stderr(std::process::Stdio::null()).output().ok()?;
    match String::from_utf8_lossy(&out.stdout).trim() {
        "on" | "all" => Some(true),
        "off" => Some(false),
        _ => None,
    }
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
    // The last frame drawn, to see what it had over images.
    let mut last: Option<ratatui::buffer::Buffer> = None;
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
                last = None;
            }
            screen = now;
        }
        let frame = terminal.draw(|f| {
            ck::draw(f, app);
            if let Some(last) = &last {
                resend_uncovered_images(last, f.buffer_mut());
            }
        })?;
        last = Some(frame.buffer.clone());
        app.show_title();
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

/// An image drawn as one escape sequence (sixel, iTerm2) is sent in its first cell, and the
/// rest of its cells are left to the terminal ("skip" cells, which ratatui never writes);
/// it's sent again only when that first cell changes. So text drawn over part of one (a
/// popup) and then taken away would stay on the image. When a cell of an image held
/// something else in the last frame, the image's first cell is changed (its color, which
/// the image doesn't use) so the image is sent again over it.
fn resend_uncovered_images(last: &ratatui::buffer::Buffer, buf: &mut ratatui::buffer::Buffer) {
    use ratatui::buffer::{Buffer, CellDiffOption};
    use ratatui::style::Color;
    if last.area != buf.area {
        return;
    }
    let area = buf.area;
    let skip = |b: &Buffer, x: u16, y: u16| b.cell((x, y)).is_some_and(|c| c.diff_option == CellDiffOption::Skip);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            // An image's first cell, unchanged (else it's sent anyway).
            let first = buf.cell((x, y)).filter(|c| c.diff_option != CellDiffOption::Skip && c.symbol().starts_with('\x1b'));
            if first.is_none() || first != last.cell((x, y)) {
                continue;
            }
            let right = (x + 1..area.right()).take_while(|&x| skip(buf, x, y)).count() as u16;
            let down = (y + 1..area.bottom()).take_while(|&y| skip(buf, x, y)).count() as u16;
            let uncovered = (y..=y + down).any(|y| (x..=x + right).any(|x| skip(buf, x, y) && !skip(last, x, y)));
            if uncovered && let Some(c) = buf.cell_mut((x, y)) {
                c.fg = if c.fg == Color::Reset { Color::Black } else { Color::Reset };
            }
        }
    }
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

    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::{Clear, Widget};

    #[test]
    fn an_image_a_closed_popup_covered_is_sent_again() {
        let area = Rect::new(0, 0, 20, 10);
        let image = |buf: &mut Buffer| {
            let mut picker = ratatui_image::picker::Picker::halfblocks();
            picker.set_protocol_type(ratatui_image::picker::ProtocolType::Sixel);
            let ratatui_image::FontSize { width: w, height: h } = picker.font_size();
            let img = image::DynamicImage::new_rgb8(6 * u32::from(w), 4 * u32::from(h));
            let p = picker.new_protocol(img, ratatui::layout::Size::new(6, 4), ratatui_image::Resize::Fit(None)).unwrap();
            ratatui_image::Image::new(&p).render(Rect::new(2, 2, 6, 4), buf);
        };
        // A popup over the lower right of the image, not its first cell.
        let mut before = Buffer::empty(area);
        image(&mut before);
        let popup = Rect::new(5, 4, 10, 3);
        Clear.render(popup, &mut before);
        before.set_string(5, 4, "a popup", ratatui::style::Style::new());
        // Closed: ratatui alone writes nothing over the image, and doesn't send it again,
        // so the popup's text would stay on it.
        let mut after = Buffer::empty(area);
        image(&mut after);
        let on_image = |b: &Buffer, a: &Buffer| -> Vec<(u16, u16)> {
            b.diff(a).iter().map(|&(x, y, _)| (x, y)).filter(|&(x, y)| (2..8).contains(&x) && (2..6).contains(&y)).collect()
        };
        assert_eq!(on_image(&before, &after), []);
        super::resend_uncovered_images(&before, &mut after);
        assert_eq!(on_image(&before, &after), [(2, 2)]);
        // An image nothing covered isn't sent again.
        let mut again = Buffer::empty(area);
        image(&mut again);
        let mut unchanged = again.clone();
        super::resend_uncovered_images(&again, &mut unchanged);
        assert_eq!(again, unchanged);
    }

    #[test]
    fn only_a_main_thread_panic_restores_the_terminal() {
        let restored = Arc::new(AtomicUsize::new(0));
        let count = restored.clone();
        std::panic::set_hook(Box::new(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
        }));
        super::restore_on_main_thread_panics();
        assert!(std::thread::spawn(|| std::panic::panic_any("deliberate, in the background")).join().is_err());
        assert_eq!(restored.load(Ordering::SeqCst), 0);
        assert!(std::panic::catch_unwind(|| std::panic::panic_any("on the main thread")).is_err());
        assert_eq!(restored.load(Ordering::SeqCst), 1);
        let _ = std::panic::take_hook();
    }
}
