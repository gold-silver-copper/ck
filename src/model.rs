//! Site-agnostic data model. Every backend converts its own JSON into these.

use std::sync::OnceLock;

use ratatui::text::Line;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Board {
    /// URI segment, e.g. `g` or `λ`.
    pub uri: String,
    pub title: String,
    /// `None` when the site doesn't say.
    pub nsfw: Option<bool>,
}

/// Where a quote link points, as far as the markup tells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// `None`: the current board.
    pub board: Option<String>,
    /// `None`: unknown, or the current thread for a bare `>>123`.
    pub thread: Option<u64>,
    /// `None` for a board link like `>>>/g/`.
    pub post: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Attachment {
    pub filename: String,
    pub url: String,
    /// Thumbnail image, if the site makes one we can show (not for spoilers or generic icons).
    pub thumb: Option<String>,
    pub spoiler: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size: Option<u64>,
}

impl Attachment {
    /// Lowercase file extension of the full file, without the dot.
    pub fn ext(&self) -> String {
        let name = self.url.rsplit('/').next().unwrap_or("");
        name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
    }

    pub fn is_video(&self) -> bool {
        matches!(self.ext().as_str(), "webm" | "mp4" | "mov" | "m4v" | "mkv")
    }

    /// Whether the full file is something we can decode and show in the terminal.
    pub fn is_image(&self) -> bool {
        matches!(self.ext().as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp")
    }
}

#[derive(Debug, Clone, Default)]
pub struct Post {
    pub no: u64,
    pub name: String,
    pub subject: Option<String>,
    /// Unix timestamp (seconds).
    pub time: i64,
    /// Comment already parsed into styled, unwrapped lines.
    pub body: Vec<Line<'static>>,
    /// Post numbers this post quotes (`>>123`).
    pub quotes: Vec<u64>,
    /// All quote links, including ones to other threads and boards.
    pub links: Vec<Link>,
    pub files: Vec<Attachment>,
    // Catalog-only fields.
    pub replies: Option<u32>,
    pub images: Option<u32>,
    pub sticky: bool,
    pub locked: bool,
    /// The board the thread is on, when the site says. Overboards mix threads from many
    /// boards, so it can differ from the board being browsed.
    pub board: Option<String>,
    /// `plain_text` and `search_text`, computed once.
    pub text: OnceLock<(String, String)>,
}

impl Post {
    /// Body flattened to a single line of plain text, for previews and filtering. Spoilers
    /// are left out. Computed once.
    pub fn plain_text(&self) -> &str {
        &self.texts().0
    }

    /// Lowercase number, subject and plain text, for case-insensitive filtering.
    pub fn search_text(&self) -> &str {
        &self.texts().1
    }

    fn texts(&self) -> &(String, String) {
        self.text.get_or_init(|| {
            let mut plain = String::new();
            for line in &self.body {
                if !plain.is_empty() {
                    plain.push(' ');
                }
                for span in &line.spans {
                    if crate::markup::is_spoiler(span.style) {
                        plain.push_str("[spoiler]");
                    } else {
                        plain.push_str(&span.content);
                    }
                }
            }
            let search = format!("{} {} {plain}", self.no, self.subject.as_deref().unwrap_or("")).to_lowercase();
            (plain, search)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::markup::{Flavor, parse_html};

    #[test]
    fn plain_text_hides_spoilers() {
        let parsed = parse_html(r#"With a <span class="spoiler">SaaS</span> of $5"#, Flavor::Vichan);
        let p = super::Post { body: parsed.lines, ..Default::default() };
        assert_eq!(p.plain_text(), "With a [spoiler] of $5");
        assert_eq!(p.search_text(), "0  with a [spoiler] of $5");
    }
}
