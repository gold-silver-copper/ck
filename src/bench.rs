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
    app.board = Some(Board { uri: "g".into(), title: "Technology".into(), nsfw: Some(false) });
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
    a.view = View::Thread;
    a.thread = Some(ThreadView::new("g".into(), posts[0].no, posts.clone()));
    let mut t = term();
    eprintln!("\n== thread, {} posts ==", posts.len());
    time("frame, layout cached", 200, || draw(&mut t, &mut a));
    time("frame with full layout rebuild", 30, || {
        a.thread.as_mut().unwrap().layout = None;
        draw(&mut t, &mut a)
    });
    time("search keystroke", 30, || a.thread.as_mut().unwrap().set_search("the".into()));
}

#[test]
#[ignore]
fn bench_catalog() {
    let v = fixture("4chan_catalog.json");
    let cat = scale(&Futaba::fourchan(None).parse_catalog("g", &v), 150);
    let mut a = app();
    a.images = Images::offline();
    a.view = View::Catalog;
    a.catalog = cat;
    a.catalog_list.state.select(Some(0));
    let mut t = term();
    eprintln!("\n== catalog, 150 threads ==");
    time("frame with thumbnail placeholders", 200, || draw(&mut t, &mut a));
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
    }
}

#[test]
#[ignore]
fn bench_images() {
    eprintln!("\n== images: UI-thread cost of the first frame showing a decoded image ==");
    for proto in [ProtocolType::Halfblocks, ProtocolType::Sixel, ProtocolType::Kitty] {
        // The viewer showing a 2048x1536 image for the first time.
        let mut worst = Duration::ZERO;
        let mut total = Duration::ZERO;
        let runs = 5;
        for _ in 0..runs {
            let mut a = app();
            a.images = Images::with_picker(picker(proto));
            a.images.insert_decoded("https://x/big.png", DynamicImage::new_rgb8(2048, 1536));
            a.viewer = Some(Viewer { files: vec![file("https://x/big.png")], index: 0 });
            let mut t = term();
            let start = Instant::now();
            draw(&mut t, &mut a);
            let d = start.elapsed();
            worst = worst.max(d);
            total += d;
        }
        report(&format!("{proto:?}: viewer, first frame (avg)"), total / runs);
        report(&format!("{proto:?}: viewer, first frame (worst)"), worst);

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
            a.view = View::Thread;
            a.thread = Some(ThreadView::new("g".into(), 100, posts.clone()));
            let mut t = term();
            let start = Instant::now();
            draw(&mut t, &mut a);
            total += start.elapsed();
        }
        report(&format!("{proto:?}: thread frame with 3 new thumbnails"), total / runs);
    }
}
