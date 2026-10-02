//! `[[filter]]` rules: hide or highlight threads and posts by regex (or file MD5).

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Deserialize;

use crate::model::Post;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Field {
    Subject,
    Comment,
    Name,
    Filename,
    /// A file's MD5 in base64 (`pattern` is compared exactly, not as a regex).
    Md5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterAction {
    #[default]
    Hide,
    Highlight,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Fields {
    One(Field),
    Many(Vec<Field>),
}

/// One `[[filter]]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterConfig {
    pub pattern: String,
    /// Default: subject and comment.
    #[serde(default)]
    pub field: Option<Fields>,
    #[serde(default)]
    pub sites: Vec<String>,
    #[serde(default)]
    pub boards: Vec<String>,
    #[serde(default)]
    pub action: FilterAction,
    #[serde(default)]
    pub label: Option<String>,
}

struct Filter {
    re: Option<Regex>,
    pattern: String,
    fields: Vec<Field>,
    sites: Vec<String>,
    boards: Vec<String>,
    action: FilterAction,
    label: String,
}

/// What filters (and manual hiding) say about a thread or post.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mark {
    /// Hidden: by the filter with this label, or by hand (`""`).
    pub hidden: Option<String>,
    pub highlight: Option<String>,
}

#[derive(Default)]
pub struct Filters(Vec<Filter>);

impl Filters {
    pub fn new(cfgs: &[FilterConfig]) -> Result<Self> {
        let mut out = Vec::new();
        for (i, c) in cfgs.iter().enumerate() {
            let fields = match &c.field {
                None => vec![Field::Subject, Field::Comment],
                Some(Fields::One(f)) => vec![*f],
                Some(Fields::Many(v)) if v.is_empty() => bail!("[[filter]] #{}: `field` is empty", i + 1),
                Some(Fields::Many(v)) => v.clone(),
            };
            let re = if fields.iter().any(|&f| f != Field::Md5) {
                Some(Regex::new(&c.pattern).with_context(|| format!("[[filter]] #{} (pattern = {:?})", i + 1, c.pattern))?)
            } else {
                None
            };
            out.push(Filter {
                re,
                pattern: c.pattern.clone(),
                fields,
                sites: c.sites.clone(),
                boards: c.boards.clone(),
                action: c.action,
                label: c.label.clone().unwrap_or_else(|| c.pattern.clone()),
            });
        }
        Ok(Self(out))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// What the filters say about a post on `site`'s `board`.
    pub fn check(&self, site: &str, board: &str, p: &Post) -> Mark {
        let mut mark = Mark::default();
        let mut comment: Option<String> = None;
        for f in &self.0 {
            let slot = match f.action {
                FilterAction::Hide => &mut mark.hidden,
                FilterAction::Highlight => &mut mark.highlight,
            };
            if slot.is_some()
                || (!f.sites.is_empty() && !f.sites.iter().any(|s| s.eq_ignore_ascii_case(site)))
                || (!f.boards.is_empty() && !f.boards.iter().any(|b| b == board))
            {
                continue;
            }
            let hit = f.fields.iter().any(|field| {
                let re = |s: &str| f.re.as_ref().is_some_and(|re| re.is_match(s));
                match field {
                    Field::Subject => p.subject.as_deref().is_some_and(re),
                    Field::Name => re(&p.name),
                    Field::Filename => p.files.iter().any(|file| re(&file.filename)),
                    Field::Md5 => p.files.iter().any(|file| file.md5.as_deref() == Some(f.pattern.as_str())),
                    // The whole comment, spoilers included, a line per line.
                    Field::Comment => re(comment.get_or_insert_with(|| {
                        p.body.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n")
                    })),
                }
            });
            if hit {
                *slot = Some(f.label.clone());
            }
        }
        mark
    }
}

#[cfg(test)]
mod tests {
    use ratatui::text::Line;

    use super::*;
    use crate::model::Attachment;

    fn filters(toml_text: &str) -> Result<Filters> {
        #[derive(Deserialize)]
        struct C {
            filter: Vec<FilterConfig>,
        }
        Filters::new(&toml::from_str::<C>(toml_text)?.filter)
    }

    fn post(subject: &str, body: &str) -> Post {
        Post {
            no: 1,
            name: "Anonymous".into(),
            subject: Some(subject.into()).filter(|s: &String| !s.is_empty()),
            body: body.lines().map(|l| Line::raw(l.to_string())).collect(),
            files: vec![Attachment { filename: "cat.png".into(), md5: Some("u8Vh17KxaDvUJ6bBcmE/eg==".into()), ..Default::default() }],
            ..Default::default()
        }
    }

    #[test]
    fn hide_highlight_fields_and_scopes() {
        let f = filters(
            r#"
            [[filter]]
            pattern = "(?i)crypto"
            label = "crypto"
            [[filter]]
            pattern = "^>be me$"
            field = "comment"
            action = "highlight"
            boards = ["g"]
            [[filter]]
            pattern = "u8Vh17KxaDvUJ6bBcmE/eg=="
            field = "md5"
            sites = ["4chan"]
            [[filter]]
            pattern = "\\.png$"
            field = ["filename", "name"]
            action = "highlight"
            label = "png"
            "#,
        )
        .unwrap();
        assert_eq!(f.len(), 4);
        // Subject, case-insensitive by the pattern's own flag.
        let m = f.check("lainchan", "b", &post("CRYPTO general", "hi"));
        assert_eq!(m.hidden.as_deref(), Some("crypto"));
        // Comments match line by line with (?m)-less anchors over the whole text; the first
        // highlight wins.
        let m = f.check("lainchan", "g", &post("", ">be me"));
        assert_eq!((m.hidden, m.highlight.as_deref()), (None, Some("^>be me$")));
        // Other boards don't get the /g/-only filter; files and names are checked.
        let m = f.check("lainchan", "b", &post("", ">be me"));
        assert_eq!(m.highlight.as_deref(), Some("png"));
        // MD5s compare exactly, on the listed sites only.
        assert_eq!(f.check("4chan", "b", &post("", "x")).hidden.as_deref(), Some("u8Vh17KxaDvUJ6bBcmE/eg=="));
    }

    #[test]
    fn bad_filters_are_reported_with_their_position() {
        let err = filters("[[filter]]\npattern = \"ok\"\n[[filter]]\npattern = \"(unclosed\"").err().unwrap();
        assert!(format!("{err:#}").contains("[[filter]] #2"), "{err:#}");
        assert!(filters("[[filter]]\npattern = \"x\"\nfield = \"nope\"").is_err());
        assert!(filters("[[filter]]\npattern = \"x\"\nfield = []").is_err());
        assert!(filters("[[filter]]\npatern = \"x\"").is_err());
    }

    #[test]
    fn checking_is_fast() {
        let toml_text: String = (0..20).map(|i| format!("[[filter]]\npattern = \"(?i)word{i}|other{i}\"\n")).collect();
        let f = filters(&toml_text).unwrap();
        let text = "lorem ipsum dolor sit amet consectetur adipiscing elit ".repeat(20);
        let posts: Vec<Post> = (0..300).map(|i| post(&format!("thread {i}"), &text)).collect();
        let start = std::time::Instant::now();
        let hidden = posts.iter().filter(|p| f.check("4chan", "g", p).hidden.is_some()).count();
        let took = start.elapsed();
        eprintln!("20 filters over 300 posts: {took:?}");
        assert_eq!(hidden, 0);
        // Run once per load, not per frame. About 1.3ms in a release build; debug is far slower.
        let limit = if cfg!(debug_assertions) { 3000 } else { 50 };
        assert!(took < std::time::Duration::from_millis(limit), "{took:?}");
    }
}
