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
    let mut app = crate::app::tests::test_app();
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
#[ignore]
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
    // The conversation of the most-replied post, and of the OP (everything, capped).
    let th = a.tab.thread.as_ref().unwrap();
    let n = crate::app::conversation_of(&th.posts, &th.index, &th.backlinks, most).0.len();
    eprintln!("(a conversation of {n} posts)");
    time("conversation of a post", 200, || crate::app::conversation_of(&th.posts, &th.index, &th.backlinks, most));
    time("conversation of the OP (500 posts at most)", 200, || crate::app::conversation_of(&th.posts, &th.index, &th.backlinks, 0));
}

#[test]
#[ignore]
fn bench_saved() {
    let posts = Futaba::fourchan(None).parse_thread("g", &fixture("4chan_thread.json"));
    let posts = scale(&posts, 1000);
    let dir = tempfile::tempdir().unwrap();
    let (mut store, _) = crate::store::Store::load(Some(dir.path().to_path_buf()));
    let key = crate::store::ThreadKey { site: "4chan".into(), board: "g".into(), no: posts[0].no };
    eprintln!("\n== saved threads, {} posts ==", posts.len());
    // New posts each time: converted, hashed and written.
    let mut n = 0;
    time("saving a watched thread as posts arrive", 20, || {
        n += 1;
        store.keep_thread(&key, "s", "u", &posts[..posts.len() - n % 2], 0).unwrap()
    });
    time("a refresh with nothing new (not written)", 20, || store.keep_thread(&key, "s", "u", &posts[..posts.len() - n % 2], 0).unwrap());
    time("opening a saved copy", 20, || {
        let t = store.load_saved(&key).unwrap();
        t.posts.into_iter().map(Post::from).collect::<Vec<_>>()
    });
}

#[test]
#[ignore]
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
    a.filters = crate::filter::tests::filters(&toml_text).unwrap();
    a.tab.catalog = scale(&a.tab.catalog, 300);
    eprintln!("\n== catalog, 300 threads, 20 filters ==");
    time("filtering (once per load)", 50, || a.remark_catalog());
    time("frame", 200, || draw(&mut t, &mut a));
}

fn picker(proto: ProtocolType) -> Picker {
    let mut p = Picker::halfblocks();
    p.set_protocol_type(proto);
    p
}

fn file(url: &str) -> Attachment {
    Attachment { filename: "x.png".into(), url: url.into(), thumb: Some(format!("{url}.thumb")), ..Default::default() }
}

#[test]
#[ignore]
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
            a.tab.viewer = Some(Viewer { files: vec![file("https://x/big.png")], index: 0, link: None });
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
