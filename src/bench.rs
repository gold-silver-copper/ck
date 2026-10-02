//! Timings of the work done on the UI thread, from fixture data scaled up. Run with
//! `cargo test --release -- --ignored bench --nocapture --test-threads=1`.

use std::time::{Duration, Instant};

use image::DynamicImage;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui_image::picker::{Picker, ProtocolType};

use crate::app::{App, ThreadView, View, Viewer};
use crate::backend::futaba::Futaba;
use crate::config::Config;
use crate::images::Images;
use crate::keys::KeyMap;
use crate::model::{Attachment, Board, Post};
use crate::store::Store;

fn fixture(name: &str) -> serde_json::Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

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
    let cfg: Config = toml::from_str(crate::config::DEFAULT_CONFIG).unwrap();
    let mut app = App::new(cfg, KeyMap::default(), None, Store::default());
    app.config_path = None;
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
    #[derive(serde::Deserialize)]
    struct C {
        filter: Vec<crate::filter::FilterConfig>,
    }
    a.filters = crate::filter::Filters::new(&toml::from_str::<C>(&toml_text).unwrap().filter).unwrap();
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
    Attachment {
        filename: "x.png".into(),
        url: url.into(),
        thumb: Some(format!("{url}.thumb")),
        spoiler: false,
        width: None,
        height: None,
        size: None,
        md5: None,
    }
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
