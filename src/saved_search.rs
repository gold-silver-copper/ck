//! Searching saved threads: every post of every saved copy, read from the data directory,
//! matched the way searching inside a thread matches (the query, lowercased, in the post's
//! name, subject, file names and text, hidden spoilers left out).

use std::collections::HashSet;

use serde::Deserialize;

use crate::model::{Attachment, Post, search_haystack};
use crate::saved::{Run, SavedLine};

/// What searching needs of a saved copy; the rest of the file is skipped.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Copy {
    posts: Vec<PostText>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PostText {
    no: u64,
    name: String,
    subject: Option<String>,
    time: i64,
    body: Vec<SavedLine>,
    files: Vec<Attachment>,
    board: Option<String>,
    quotes: Vec<u64>,
}

fn is_spoiler(r: &Run) -> bool {
    r.kind == "spoiler" && r.bg == "spoiler"
}

/// The text of saved lines, as `Post::plain_text` has it.
fn plain(body: &[SavedLine]) -> String {
    let mut out = String::new();
    for line in body {
        if !out.is_empty() {
            out.push(' ');
        }
        for r in &line.runs {
            out.push_str(if is_spoiler(r) { "[spoiler]" } else { &r.text });
        }
    }
    out
}

/// The posts of a saved copy (its file's bytes) that match `needle` (lowercase, not empty),
/// and with them what decides whether they're hidden (`with_ancestry`).
pub fn matching(bytes: &[u8], needle: &str) -> anyhow::Result<(Vec<Post>, Vec<Post>)> {
    let copy: Copy = serde_json::from_slice(bytes)?;
    let hay = |p: &PostText| search_haystack(&p.name, p.subject.as_deref(), p.files.iter().map(|f| f.filename.as_str()), &plain(&p.body)).contains(needle);
    let found: HashSet<u64> = copy.posts.iter().filter(|p| hay(p)).map(|p| p.no).collect();
    if found.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let post = |p: PostText| {
        let body = p.body.into_iter().map(|l| ratatui::text::Line::from(l.runs.into_iter().map(ratatui::text::Span::from).collect::<Vec<_>>())).collect();
        Post { no: p.no, name: p.name, subject: p.subject, time: p.time, body, files: p.files, board: p.board, quotes: p.quotes, ..Default::default() }
    };
    let mut posts: Vec<Post> = copy.posts.into_iter().map(post).collect();
    let context = crate::app::with_ancestry(&posts, |p| found.contains(&p.no));
    posts.retain(|p| found.contains(&p.no));
    Ok((posts, context))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saved::SavedPost;

    #[test]
    fn saved_text_is_the_threads_text() {
        // Every fixture post: what searching a saved copy reads is what searching the thread
        // reads.
        for (site, posts) in crate::backend::fixture_threads() {
            for p in &posts {
                let saved = SavedPost::from(p);
                assert_eq!(plain(&saved.body), p.plain_text(), "{site} No.{}", p.no);
            }
        }
    }

    #[test]
    fn finds_posts_like_thread_search() {
        let posts: Vec<Post> = crate::backend::fixture_threads().into_iter().flat_map(|(_, p)| p).collect();
        let file = serde_json::json!({ "version": 1, "site": "s", "board": "b", "no": 1, "posts": posts.iter().map(SavedPost::from).collect::<Vec<_>>() });
        let bytes = serde_json::to_vec(&file).unwrap();
        for needle in ["the", "a", "http", "[spoiler]", "zzzz-nothing", "λ", ">>"] {
            let found: Vec<u64> = matching(&bytes, needle).unwrap().0.iter().map(|p| p.no).collect();
            let brute: Vec<u64> = posts
                .iter()
                .filter(|p| search_haystack(&p.name, p.subject.as_deref(), p.files.iter().map(|f| f.filename.as_str()), p.plain_text()).contains(needle))
                .map(|p| p.no)
                .collect();
            assert_eq!(found, brute, "{needle}");
        }
        assert!(matching(b"not json", "x").is_err());
    }

    #[test]
    fn random_threads_against_brute_force() {
        use crate::fuzz::{Rng, html};
        use crate::markup::{Flavor, parse_html};
        for seed in 0..60 {
            let mut rng = Rng::new(seed);
            let posts: Vec<Post> = (0..1 + rng.below(30))
                .map(|k| {
                    let (pieces, flavor) = (1 + rng.below(12), *rng.pick(&[Flavor::Vichan, Flavor::Lynxchan]));
                    let mut p: Post = parse_html(&html(&mut rng, pieces), flavor).into();
                    p.no = k as u64 + 1;
                    p.name = rng.pick(&["Anonymous", "Ünïcödé", "", "name"]).to_string();
                    if rng.chance(30) {
                        p.subject = Some(html(&mut rng, 2));
                    }
                    p
                })
                .collect();
            let file = serde_json::json!({ "posts": posts.iter().map(SavedPost::from).collect::<Vec<_>>() });
            let bytes = serde_json::to_vec(&file).unwrap();
            let hay = |p: &Post| search_haystack(&p.name, p.subject.as_deref(), p.files.iter().map(|f| f.filename.as_str()), p.plain_text());
            // Needles: pieces of the posts' own text, and a few words.
            let mut needles: Vec<String> = ["a", "e", "spoiler", "ü", "the", " "].iter().map(|s| s.to_string()).collect();
            for p in posts.iter().take(5) {
                let h: Vec<char> = hay(p).chars().collect();
                if h.len() > 3 {
                    let (at, len) = (rng.below(h.len() - 3), 1 + rng.below(3));
                    needles.push(h[at..(at + len).min(h.len())].iter().collect());
                }
            }
            for needle in &needles {
                let found: Vec<u64> = matching(&bytes, needle).unwrap().0.iter().map(|p| p.no).collect();
                let brute: Vec<u64> = posts.iter().filter(|p| hay(p).contains(needle.as_str())).map(|p| p.no).collect();
                assert_eq!(found, brute, "seed {seed}, {needle:?}");
            }
        }
    }
}
