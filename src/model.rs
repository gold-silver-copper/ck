//! Site-agnostic data model. Every backend converts its own JSON into these.

use ratatui::text::Line;

#[derive(Debug, Clone)]
pub struct Board {
    /// URI segment, e.g. `g` or `λ`.
    pub uri: String,
    pub title: String,
    /// `None` when the site doesn't say.
    pub nsfw: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct Attachment {
    pub filename: String,
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size: Option<u64>,
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
    pub files: Vec<Attachment>,
    // Catalog-only fields.
    pub replies: Option<u32>,
    pub images: Option<u32>,
    pub sticky: bool,
    pub locked: bool,
}

impl Post {
    /// Body flattened to a single line of plain text, for previews and filtering.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for line in &self.body {
            if !out.is_empty() {
                out.push(' ');
            }
            for span in &line.spans {
                out.push_str(&span.content);
            }
        }
        out
    }
}
