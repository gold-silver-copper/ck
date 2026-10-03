//! Imageboard backends. Each one speaks a different engine's JSON API.

pub mod detect;
pub(crate) mod foolfuuka;
pub(crate) mod futaba;
mod jschan;
mod lynxchan;
mod makaba;

use std::sync::Arc;

use anyhow::Result;

use crate::config::{BoardConfig, SiteConfig, SiteKind};
use crate::model::{Board, Post};

/// Receives the results so far while a multi-page load continues.
pub type Partial<'a, T> = &'a dyn Fn(&[T]);

/// A page of search results: `(thread, post)` pairs, and how many there are in all.
#[derive(Default)]
pub struct SearchPage {
    pub hits: Vec<(u64, Post)>,
    pub total: Option<u64>,
}

pub trait Backend: Send + Sync {
    /// All boards. Multi-page lists report each page through `partial` as it arrives.
    fn boards(&self, partial: Partial<Board>) -> Result<Vec<Board>>;
    /// Thread OPs on a board, in catalog order, reporting pages through `partial`.
    fn catalog(&self, board: &str, partial: Partial<Post>) -> Result<Vec<Post>>;
    /// All posts of a thread, OP first.
    fn thread(&self, board: &str, no: u64) -> Result<Vec<Post>>;
    /// The thread a post is in, for engines that can look it up.
    fn find_thread(&self, _board: &str, _post: u64) -> Result<Option<u64>> {
        Ok(None)
    }
    fn board_url(&self, board: &str) -> String;
    fn thread_url(&self, board: &str, no: u64) -> String;
    /// Search a board's posts (archives can). Pages count from 1.
    fn search(&self, _board: &str, _query: &str, _page: u32) -> Result<SearchPage> {
        anyhow::bail!("This site can't be searched")
    }
    /// A link to one post of a thread.
    fn post_url(&self, board: &str, thread: u64, post: u64) -> String {
        format!("{}#{post}", self.thread_url(board, thread))
    }
}

pub fn build(cfg: &SiteConfig) -> Arc<dyn Backend> {
    let url = cfg.url.as_deref().map(|u| u.trim_end_matches('/').to_string());
    let boards = cfg.boards.as_ref().map(|bs| bs.iter().map(to_board).collect());
    match cfg.kind {
        SiteKind::Fourchan => Arc::new(futaba::Futaba::fourchan(boards)),
        SiteKind::Vichan => {
            Arc::new(futaba::Futaba::vichan(url.unwrap_or_default(), cfg.thumb_ext.clone(), cfg.media_url.clone(), boards))
        }
        SiteKind::Lynxchan => Arc::new(lynxchan::Lynxchan::new(url.unwrap_or_default(), boards)),
        SiteKind::Foolfuuka => Arc::new(foolfuuka::Foolfuuka::new(url.unwrap_or_default(), boards)),
        SiteKind::Jschan => Arc::new(jschan::Jschan::new(url.unwrap_or_default(), boards)),
        SiteKind::Makaba => Arc::new(makaba::Makaba::new(url.unwrap_or_default(), cfg.media_url.clone(), boards)),
    }
}

pub fn to_board(b: &BoardConfig) -> Board {
    match b {
        BoardConfig::Uri(uri) => Board { uri: uri.clone(), title: String::new(), nsfw: None },
        BoardConfig::Full { uri, title } => Board { uri: uri.clone(), title: title.clone(), nsfw: None },
    }
}

/// Every engine's parsers on one JSON value, for fuzzing: none may panic, whatever it is.
#[doc(hidden)]
pub fn parse_everything(v: &serde_json::Value) -> Vec<Post> {
    let base = "https://fuzz.invalid";
    let (vichan, fourchan) = (futaba::Futaba::vichan(base.into(), None, None, None), futaba::Futaba::fourchan(None));
    let lynx = lynxchan::Lynxchan::new(base.into(), None);
    let mak = makaba::Makaba::new(base.into(), None, None);
    let _ = (futaba::parse_boards(v), foolfuuka::parse_archives(v), jschan::parse_boards(v), lynxchan::parse_boards(v));
    let _ = (lynxchan::parse_overboards(v), makaba::parse_boards(v));
    let mut posts = Vec::new();
    for b in [&vichan, &fourchan] {
        posts.extend(b.parse_catalog("g", v));
        posts.extend(b.parse_thread("g", v));
    }
    posts.extend(foolfuuka::parse_index(v));
    posts.extend(foolfuuka::parse_thread(v));
    posts.extend(foolfuuka::parse_search(v).map(|p| p.hits.into_iter().map(|(_, p)| p).collect::<Vec<_>>()).unwrap_or_default());
    posts.extend(jschan::parse_overboard(base, v));
    posts.extend(jschan::parse_thread(base, v));
    posts.extend(lynx.parse_catalog(v));
    posts.extend(lynx.parse_index(v));
    posts.extend(lynx.parse_thread(v));
    posts.extend(mak.parse_catalog(v));
    posts.extend(mak.parse_thread(v));
    for p in &posts {
        let _ = (p.plain_text(), p.search_text());
    }
    posts
}

/// A JSON file from `tests/fixtures`.
#[cfg(test)]
pub fn fixture(name: &str) -> serde_json::Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Every fixture thread, parsed by its backend.
#[cfg(test)]
pub fn fixture_threads() -> Vec<(&'static str, Vec<Post>)> {
    use futaba::Futaba;
    let fourchan = Futaba::fourchan(None);
    let lain = Futaba::vichan("https://lainchan.org".into(), Some("png".into()), None, None);
    let leftypol = Futaba::vichan("https://leftypol.org".into(), None, None, None);
    let end = lynxchan::Lynxchan::new("https://endchan.net".into(), None);
    let kohl = lynxchan::Lynxchan::new("https://kohlchan.net".into(), None);
    let dvach = makaba::Makaba::new("https://2ch.hk".into(), Some("https://2ch.su".into()), None);
    vec![
        ("4chan", fourchan.parse_thread("g", &fixture("4chan_thread.json"))),
        ("vichan", lain.parse_thread("λ", &fixture("vichan_thread.json"))),
        ("leftypol", leftypol.parse_thread("leftypol", &fixture("leftypol_thread.json"))),
        ("lynxchan", end.parse_thread(&fixture("lynxchan_thread.json"))),
        ("kohlchan", kohl.parse_thread(&fixture("kohlchan_thread.json"))),
        ("makaba", dvach.parse_thread(&fixture("makaba_thread.json"))),
        ("jschan", jschan::parse_thread("https://zzzchan.xyz", &fixture("jschan_thread.json"))),
        ("foolfuuka", foolfuuka::parse_thread(&fixture("foolfuuka_thread.json"))),
    ]
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    /// Hits the network: `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_default_sites() {
        crate::http::NETWORK.store(true, std::sync::atomic::Ordering::Relaxed);
        let cfg: Config = toml::from_str(crate::config::DEFAULT_CONFIG).unwrap();
        let mut failures = Vec::new();
        for site in &cfg.sites {
            let b = super::build(site);
            let result = (|| -> anyhow::Result<String> {
                let pages = std::cell::Cell::new(0);
                let boards = b.boards(&|_| pages.set(pages.get() + 1))?;
                let board = &boards.first().ok_or_else(|| anyhow::anyhow!("no boards"))?.uri;
                let cat = b.catalog(board, &|_| pages.set(pages.get() + 1))?;
                let op = cat.iter().find(|p| !p.sticky).or(cat.first()).ok_or_else(|| anyhow::anyhow!("empty catalog"))?;
                // On an overboard the thread is on its own board; then also try a real board.
                let thread_board = op.board.as_deref().unwrap_or(board);
                if thread_board != board
                    && let Some(real) = boards.get(1)
                {
                    anyhow::ensure!(!b.catalog(&real.uri, &|_| {})?.is_empty(), "empty catalog on /{}/", real.uri);
                }
                let board = thread_board;
                let posts = b.thread(board, op.no)?;
                anyhow::ensure!(!posts.is_empty() && posts[0].no == op.no, "thread mismatch");
                // Engines that can look up a post's thread must find this one.
                if let Some(reply) = posts.get(1) {
                    let found = b.find_thread(board, reply.no)?;
                    anyhow::ensure!(found.is_none_or(|t| t == op.no), "find_thread({}) gave {found:?}", reply.no);
                }
                let files: usize = posts.iter().map(|p| p.files.len()).sum();
                // The first thumbnail must exist and decode.
                let thumb = posts.iter().flat_map(|p| &p.files).find_map(|f| f.thumb.clone());
                if let Some(url) = &thumb {
                    let bytes = crate::http::get_bytes(url, 4 * 1024 * 1024)?;
                    image::load_from_memory(&bytes).map_err(|e| anyhow::anyhow!("thumbnail {url}: {e}"))?;
                }
                Ok(format!(
                    "{} boards, /{board}/ {} threads, {} partial pages, thread {} has {} posts / {files} files; first thumb: {:?}",
                    boards.len(),
                    cat.len(),
                    pages.get(),
                    op.no,
                    posts.len(),
                    thumb
                ))
            })();
            match result {
                Ok(s) => println!("OK   {}: {s}", site.name),
                Err(e) => {
                    println!("FAIL {}: {e:#}", site.name);
                    failures.push(site.name.clone());
                }
            }
        }
        assert!(failures.is_empty(), "failed: {failures:?}");
    }
}
