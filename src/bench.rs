//! Timings of the work done on the UI thread, from fixture data scaled up. Run with
//! `cargo test --release -- --ignored bench --nocapture --test-threads=1`.

use std::time::{Duration, Instant};

use image::DynamicImage;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui_image::picker::{Picker, ProtocolType};

use crate::app::{App, ThreadView, View, Viewer};
use crate::backend::fixture;
use crate::backend::futaba::Futaba;
use crate::images::Images;
use crate::model::{Attachment, Board, Post};

fn report(label: &str, per: Duration) {
    eprintln!("{label:<60} {per:>10.2?}");
}

/// Average time of `f` over `n` runs, after one warm-up run.
fn time<T>(label: &str, n: u32, mut f: impl FnMut() -> T) -> Duration {
    f();
    let start = Instant::now();
    for _ in 0..n {
        std::hint::black_box(f());
    }
    let per = start.elapsed() / n;
    report(label, per);
    per
}

fn app() -> App {
    let mut app = crate::test_fixtures::test_app();
    app.tab.board = Some(Board { uri: "g".into(), title: "Technology".into(), nsfw: Some(false) });
    app
}

/// `base` repeated until there are about `n` items, with fresh post numbers.
fn scale(base: &[Post], n: usize) -> Vec<Post> {
    (0..n).map(|i| Post { no: base[0].no + i as u64, ..base[i % base.len()].clone() }).collect()
}

fn term() -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(110, 32)).unwrap()
}

fn draw(term: &mut Terminal<TestBackend>, app: &mut App) {
    term.draw(|f| crate::ui::draw(f, app)).unwrap();
}

#[test]
#[ignore = "benchmark: timings, best run in release"]
fn bench_thread() {
    let posts = Futaba::fourchan(None).parse_thread("g", &fixture("4chan_thread.json"));
    let posts = scale(&posts, 1000);
    let mut a = app();
    a.tab.view = View::Thread;
    a.tab.thread = Some(ThreadView::new("g".into(), posts[0].no, posts.clone()));
    let mut t = term();
    eprintln!("\n== thread, {} posts ==", posts.len());
    time("frame, layout cached", 200, || draw(&mut t, &mut a));
    time("frame with full layout rebuild", 30, || {
        a.tab.thread.as_mut().unwrap().layout = None;
        draw(&mut t, &mut a)
    });
    time("search keystroke", 30, || a.tab.thread.as_mut().unwrap().set_search("the".into()));
    // Matching posts are laid out again (their highlight changes); the rest aren't.
    for (label, words) in [("search keystroke, then a frame (most posts match)", ["the", "they"]), ("search keystroke, then a frame (few match)", ["linux", "linu"])] {
        let mut q = 0;
        time(label, 30, || {
            q += 1;
            a.tab.thread.as_mut().unwrap().set_search(words[q % 2].into());
            draw(&mut t, &mut a)
        });
    }
    a.tab.thread.as_mut().unwrap().set_search(String::new());
    // `e` on the most-replied post, then the frame that lays the thread out again.
    let th = a.tab.thread.as_mut().unwrap();
    let most = (0..th.posts.len()).max_by_key(|&i| th.backlinks[i].len()).unwrap();
    eprintln!("(expanding {} replies)", th.backlinks[most].len());
    th.selected = most;
    time("expand / collapse replies inline, then a frame", 30, || {
        a.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char('e')));
        draw(&mut t, &mut a)
    });
    // The same on ten times the thread: the cost should be the screen's, not the thread's.
    let big = scale(&posts, 10_000);
    let mut b = app();
    b.tab.view = View::Thread;
    b.tab.thread = Some(ThreadView::new("g".into(), big[0].no, big.clone()));
    eprintln!("\n== thread, {} posts ==", big.len());
    draw(&mut t, &mut b);
    time("10k: frame, layout cached", 200, || draw(&mut t, &mut b));
    time("10k: frame with full layout rebuild", 30, || {
        b.tab.thread.as_mut().unwrap().layout = None;
        draw(&mut t, &mut b)
    });
    let mut q = 0;
    time("10k: search keystroke, then a frame (most posts match)", 30, || {
        q += 1;
        b.tab.thread.as_mut().unwrap().set_search(["the", "they"][q % 2].into());
        draw(&mut t, &mut b)
    });
    b.tab.thread.as_mut().unwrap().set_search(String::new());
    let mut end = false;
    time("10k: G / g (to the end and back), then a frame", 30, || {
        end = !end;
        b.on_key(ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char(if end { 'G' } else { 'g' })));
        draw(&mut t, &mut b)
    });
    // The conversation of the most-replied post, and of the OP (everything, capped).
    let th = a.tab.thread.as_ref().unwrap();
    let n = crate::app::conversation_of(&th.posts, &th.index, &th.backlinks, most).0.len();
    eprintln!("(a conversation of {n} posts)");
    time("conversation of a post", 200, || crate::app::conversation_of(&th.posts, &th.index, &th.backlinks, most));
    time("conversation of the OP (500 posts at most)", 200, || crate::app::conversation_of(&th.posts, &th.index, &th.backlinks, 0));
}

#[test]
#[ignore = "benchmark: timings, best run in release"]
fn bench_saved() {
    let posts = Futaba::fourchan(None).parse_thread("g", &fixture("4chan_thread.json"));
    let posts = scale(&posts, 1000);
    let dir = tempfile::tempdir().unwrap();
    let (mut store, _) = crate::store::Store::load(Some(dir.path().to_path_buf()));
    let key = crate::store::ThreadKey { site: "4chan".into(), board: "g".into(), no: posts[0].no };
    eprintln!("\n== saved threads, {} posts ==", posts.len());
    // New posts each time: converted, hashed and written.
    let mut n = 0;
    let both = [crate::test_fixtures::whole(&posts), crate::test_fixtures::whole(&posts[..posts.len() - 1])];
    time("saving a watched thread as posts arrive", 20, || {
        n += 1;
        store.keep_thread(&key, "s", "u", &both[n % 2], 0)
    });
    time("a refresh with nothing new (not written)", 20, || store.keep_thread(&key, "s", "u", &both[n % 2], 0));
    // The work the writer thread does for each (off the UI thread).
    store.flush(std::time::Duration::from_secs(30));
    time("saving: the background writer's part", 20, || {
        n += 1;
        store.keep_thread(&key, "s", "u", &both[n % 2], 0);
        store.flush(std::time::Duration::from_secs(30))
    });
    time("opening a saved copy", 20, || {
        let t = store.load_saved(&key).unwrap();
        t.posts.into_iter().map(Post::from).collect::<Vec<_>>()
    });
}

#[test]
#[ignore = "benchmark: timings, best run in release"]
fn bench_catalog() {
    let v = fixture("4chan_catalog.json");
    let cat = scale(&Futaba::fourchan(None).parse_catalog("g", &v), 150);
    let mut a = app();
    a.images = Images::offline();
    a.tab.view = View::Catalog;
    a.tab.catalog = cat;
    a.tab.catalog_list.state.select(Some(0));
    let mut t = term();
    eprintln!("\n== catalog, 150 threads ==");
    time("frame with thumbnail placeholders", 200, || draw(&mut t, &mut a));

    // Filters run once per load; frames only look the results up.
    let toml_text: String = (0..20).map(|i| format!("[[filter]]\npattern = \"(?i)word{i}|other{i}\"\n")).collect();
    a.rehide(|a| a.hiding.set_filters(crate::filter::tests::filters(&toml_text).unwrap()));
    a.tab.catalog = scale(&a.tab.catalog, 300);
    eprintln!("\n== catalog, 300 threads, 20 filters ==");
    time("filtering (once per load)", 50, || a.remark());
    time("frame", 200, || draw(&mut t, &mut a));
}

/// Encoding on the encoder thread: how long until a decoded image can be shown, for each
/// protocol, at the default cell size and at a large real one (25x51 px, as a big font on a
/// high-density screen reports).
#[test]
#[ignore = "benchmark: timings, best run in release"]
fn bench_encoding() {
    use crate::images::Crop;
    use ratatui::layout::Size;
    eprintln!("\n== images: encoding (the encoder thread) ==");
    // A photo-like image (encoders and filters do more with detail than with flat color).
    let photo = DynamicImage::ImageRgb8(image::RgbImage::from_fn(2048, 2048, |x, y| image::Rgb([(x ^ y) as u8, (x * 3 + y) as u8, (y * 7) as u8])));
    let thumb = photo.thumbnail(250, 250);
    let zoom = Crop::FIT.zoomed(true).zoomed(true);
    for proto in [ProtocolType::Halfblocks, ProtocolType::Sixel, ProtocolType::Kitty] {
        for (cells, font) in [("10x20", (10u16, 20u16)), ("25x51", (25, 51))] {
            #[allow(deprecated)]
            let mut p = Picker::from_fontsize(font.into());
            p.set_protocol_type(proto);
            let n = if proto == ProtocolType::Halfblocks { 10 } else { 3 };
            time(&format!("{proto:?}, cells {cells}: a 2048px image in the viewer"), n, || crate::images::encode_crop(&p, &photo, Size::new(117, 30), Crop::FIT));
            time(&format!("{proto:?}, cells {cells}: a thumbnail tile"), n, || crate::images::encode_crop(&p, &thumb, Size::new(16, 8), Crop::FIT));
            time(&format!("{proto:?}, cells {cells}: the viewer zoomed to 200%"), n, || crate::images::encode_crop(&p, &photo, Size::new(117, 30), zoom));
        }
    }
}

fn picker(proto: ProtocolType) -> Picker {
    let mut p = Picker::halfblocks();
    p.set_protocol_type(proto);
    p
}

fn file(url: &str) -> Attachment {
    Attachment { filename: "x.png".into(), thumb: Some(format!("{url}.thumb")), ..Attachment::at(url) }
}

#[test]
#[ignore = "benchmark: timings, best run in release"]
fn bench_images() {
    eprintln!("\n== images: UI-thread cost of the first frame showing a decoded image ==");
    for proto in [ProtocolType::Halfblocks, ProtocolType::Sixel, ProtocolType::Kitty] {
        // The viewer showing a 2048x1536 image for the first time, and the frame that first
        // draws the finished encoding (encoding itself runs on the encoder thread).
        let mut worst = Duration::ZERO;
        let mut total = Duration::ZERO;
        let mut shown = Duration::ZERO;
        let runs = 5;
        for _ in 0..runs {
            let mut a = app();
            a.images = Images::with_picker(picker(proto));
            a.images.insert_decoded("https://x/big.png", DynamicImage::new_rgb8(2048, 1536));
            a.tab.popup = Some(crate::app::TabPopup::Viewer(Viewer::new(vec![file("https://x/big.png")], 0, None)));
            let mut t = term();
            let start = Instant::now();
            draw(&mut t, &mut a);
            let d = start.elapsed();
            worst = worst.max(d);
            total += d;
            // Wait for the encoder, then time the frame that shows the image.
            for _ in 0..500 {
                a.images.poll();
                let text: String = t.backend().buffer().content().iter().map(|c| c.symbol()).collect();
                if !text.contains("Rendering") {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
                draw(&mut t, &mut a);
            }
            let start = Instant::now();
            draw(&mut t, &mut a);
            shown += start.elapsed();
        }
        report(&format!("{proto:?}: viewer, first frame (avg)"), total / runs);
        report(&format!("{proto:?}: viewer, first frame (worst)"), worst);
        report(&format!("{proto:?}: viewer, frame showing the encoded image"), shown / runs);

        // A thread screen where 3 thumbnails just finished loading.
        let posts: Vec<Post> = (0..3)
            .map(|i| Post { no: 100 + i, files: vec![file(&format!("https://x/{i}.png"))], ..Default::default() })
            .collect();
        let mut total = Duration::ZERO;
        for _ in 0..runs {
            let mut a = app();
            a.images = Images::with_picker(picker(proto));
            for i in 0..3 {
                a.images.insert_decoded(&format!("https://x/{i}.png.thumb"), DynamicImage::new_rgb8(250, 250));
            }
            a.tab.view = View::Thread;
            a.tab.thread = Some(ThreadView::new("g".into(), 100, posts.clone()));
            let mut t = term();
            let start = Instant::now();
            draw(&mut t, &mut a);
            total += start.elapsed();
        }
        report(&format!("{proto:?}: thread frame with 3 new thumbnails"), total / runs);
    }
}

/// Searching saved threads (`:saved WORDS`): about 500 MB of saved copies, the most
/// `saved_max_mb` keeps by default, read the way the search's thread reads them.
#[test]
#[ignore = "benchmark: timings, best run in release"]
fn bench_saved_search() {
    use crate::saved::{SavedPost, SavedThread};
    use crate::store::ThreadKey;
    eprintln!("== searching saved threads, 500 MB ==");
    let base = Futaba::fourchan(None).parse_thread("g", &fixture("4chan_thread.json"));
    let posts: Vec<SavedPost> = scale(&base, 1000).iter().map(SavedPost::from).collect();
    let dir = tempfile::tempdir().unwrap();
    let one = serde_json::to_vec(&SavedThread { version: 1, site: "s".into(), board: "g".into(), no: 1, subject: String::new(), saved: 0, dead: false, url: String::new(), posts: posts.clone() }).unwrap();
    let copies = (500 << 20) / one.len() + 1;
    let keys: Vec<ThreadKey> = (0..copies as u64).map(|no| ThreadKey { site: "s".into(), board: "g".into(), no: no + 1 }).collect();
    for k in &keys {
        let t = SavedThread { version: 1, site: k.site.clone(), board: k.board.clone(), no: k.no, subject: String::new(), saved: 0, dead: false, url: String::new(), posts: posts.clone() };
        crate::saved::write(dir.path(), &t).unwrap();
    }
    eprintln!("({copies} copies of {} KB, {} MB)", one.len() >> 10, (one.len() * copies) >> 20);
    for (label, needle) in [("a word in every copy", "the"), ("a word in none", "zqxjkw")] {
        let start = Instant::now();
        let mut first = None;
        for k in &keys {
            let bytes = std::fs::read(crate::saved::path(dir.path(), k)).unwrap();
            let (hits, _) = crate::saved_search::matching(&bytes, needle).unwrap();
            if first.is_none() && !hits.is_empty() {
                first = Some(start.elapsed());
            }
        }
        if let Some(f) = first {
            report(&format!("saved search, {label}: first results"), f);
        }
        report(&format!("saved search, {label}: every copy"), start.elapsed());
    }
}

/// Hidden words: marking a 1000-post thread, a frame, and a search keystroke, with 50 words
/// against none.
#[test]
#[ignore = "benchmark: timings, best run in release"]
fn bench_hidden_words() {
    eprintln!("== hidden words, thread of 1000 posts ==");
    let posts = Futaba::fourchan(None).parse_thread("g", &fixture("4chan_thread.json"));
    let posts = scale(&posts, 1000);
    let words: Vec<String> = (0..50).map(|i| format!("word{i}")).chain(["the end".into(), "c++".into()]).take(50).collect();
    for (label, words) in [("no hidden words", Vec::new()), ("50 hidden words", words)] {
        let mut a = app();
        a.tab.view = View::Thread;
        a.rehide(|a| a.hiding.set_filters(crate::filter::Filters::new(&[]).unwrap().with_words(&words).unwrap()));
        a.tab.thread = Some(ThreadView::new("g".into(), posts[0].no, posts.clone()));
        let mut t = term();
        time(&format!("{label}: marking the thread"), 20, || a.remark());
        time(&format!("{label}: frame with full layout rebuild"), 50, || {
            if let Some(th) = &mut a.tab.thread {
                th.layout = None;
            }
            draw(&mut t, &mut a);
        });
        let mut k = 0;
        time(&format!("{label}: search keystroke, then a frame"), 50, || {
            k += 1;
            if let Some(th) = &mut a.tab.thread {
                th.set_search(if k % 2 == 0 { "th".into() } else { "the".into() });
            }
            draw(&mut t, &mut a);
        });
    }
}
