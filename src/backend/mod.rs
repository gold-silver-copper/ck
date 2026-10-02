//! Imageboard backends. Each one speaks a different engine's JSON API.

mod foolfuuka;
pub(crate) mod futaba;
mod jschan;
mod lynxchan;

use std::sync::Arc;

use anyhow::Result;

use crate::config::{BoardConfig, SiteConfig, SiteKind};
use crate::model::{Board, Post};

pub trait Backend: Send + Sync {
    fn boards(&self) -> Result<Vec<Board>>;
    /// Thread OPs on a board, in catalog order.
    fn catalog(&self, board: &str) -> Result<Vec<Post>>;
    /// All posts of a thread, OP first.
    fn thread(&self, board: &str, no: u64) -> Result<Vec<Post>>;
    /// The thread a post is in, for engines that can look it up.
    fn find_thread(&self, _board: &str, _post: u64) -> Result<Option<u64>> {
        Ok(None)
    }
    fn board_url(&self, board: &str) -> String;
    fn thread_url(&self, board: &str, no: u64) -> String;
}

pub fn build(cfg: &SiteConfig) -> Arc<dyn Backend> {
    let url = cfg.url.as_deref().map(|u| u.trim_end_matches('/').to_string());
    let boards = cfg.boards.as_ref().map(|bs| bs.iter().map(to_board).collect());
    match cfg.kind {
        SiteKind::Fourchan => Arc::new(futaba::Futaba::fourchan(boards)),
        SiteKind::Vichan => Arc::new(futaba::Futaba::vichan(url.unwrap_or_default(), cfg.thumb_ext.clone(), boards)),
        SiteKind::Lynxchan => Arc::new(lynxchan::Lynxchan::new(url.unwrap_or_default(), boards)),
        SiteKind::Foolfuuka => Arc::new(foolfuuka::Foolfuuka::new(url.unwrap_or_default(), boards)),
        SiteKind::Jschan => Arc::new(jschan::Jschan::new(url.unwrap_or_default(), boards)),
    }
}

fn to_board(b: &BoardConfig) -> Board {
    match b {
        BoardConfig::Uri(uri) => Board { uri: uri.clone(), title: String::new(), nsfw: None },
        BoardConfig::Full { uri, title } => Board { uri: uri.clone(), title: title.clone(), nsfw: None },
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    /// Hits the network: `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_default_sites() {
        let cfg: Config = toml::from_str(crate::config::DEFAULT_CONFIG).unwrap();
        let mut failures = Vec::new();
        for site in &cfg.sites {
            let b = super::build(site);
            let result = (|| -> anyhow::Result<String> {
                let boards = b.boards()?;
                let board = &boards.first().ok_or_else(|| anyhow::anyhow!("no boards"))?.uri;
                let cat = b.catalog(board)?;
                let op = cat.iter().find(|p| !p.sticky).or(cat.first()).ok_or_else(|| anyhow::anyhow!("empty catalog"))?;
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
                    "{} boards, /{board}/ {} threads, thread {} has {} posts / {files} files; first thumb: {:?}",
                    boards.len(),
                    cat.len(),
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
