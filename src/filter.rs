//! `[[filter]]` rules: hide or highlight threads and posts by regex (or file MD5).

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Deserialize;

use crate::model::Post;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Fields {
    One(Field),
    Many(Vec<Field>),
}

/// One `[[filter]]` table.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
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
    /// `false` keeps the filter in the config without applying it.
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl Field {
    pub const ALL: [Field; 5] = [Field::Subject, Field::Comment, Field::Name, Field::Filename, Field::Md5];

    pub fn as_str(self) -> &'static str {
        match self {
            Field::Subject => "subject",
            Field::Comment => "comment",
            Field::Name => "name",
            Field::Filename => "filename",
            Field::Md5 => "md5",
        }
    }
}

impl FilterAction {
    pub fn as_str(self) -> &'static str {
        match self {
            FilterAction::Hide => "hide",
            FilterAction::Highlight => "highlight",
        }
    }
}

impl FilterConfig {
    pub fn new(pattern: String, fields: &[Field]) -> Self {
        let mut c = FilterConfig { pattern, field: None, sites: Vec::new(), boards: Vec::new(), action: FilterAction::Hide, label: None, enabled: true };
        c.set_fields(fields);
        c
    }

    /// The fields it looks at (subject and comment unless it says).
    pub fn fields(&self) -> Vec<Field> {
        match &self.field {
            None => vec![Field::Subject, Field::Comment],
            Some(Fields::One(f)) => vec![*f],
            Some(Fields::Many(v)) => v.clone(),
        }
    }

    /// Set the fields: the default left unsaid, one as a string, more as a list.
    pub fn set_fields(&mut self, fields: &[Field]) {
        self.field = match fields {
            [Field::Subject, Field::Comment] => None,
            [f] => Some(Fields::One(*f)),
            v => Some(Fields::Many(v.to_vec())),
        };
    }

    /// The label shown on what it marks.
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.pattern)
    }

    /// Whether it's a filter ck can use (its pattern compiles, it has fields).
    pub fn check(&self) -> Result<()> {
        Filters::new(std::slice::from_ref(&FilterConfig { enabled: true, ..self.clone() })).map(|_| ())
    }

    /// Write it into a `[[filter]]` table: only what differs from `old` (the table as it
    /// was), so the rest keeps its comments and layout.
    pub fn write(&self, t: &mut toml_edit::Table, old: Option<&FilterConfig>) {
        use toml_edit::{Array, value};
        let list = |v: &[String]| value(v.iter().map(String::as_str).collect::<Array>());
        let changed = |f: &dyn Fn(&FilterConfig) -> bool| old.is_none_or(|o| !f(o));
        if changed(&|o| o.pattern == self.pattern) {
            t["pattern"] = value(self.pattern.as_str());
        }
        if changed(&|o| o.field == self.field) {
            match &self.field {
                None => {
                    t.remove("field");
                }
                Some(Fields::One(f)) => t["field"] = value(f.as_str()),
                Some(Fields::Many(v)) => t["field"] = value(v.iter().map(|f| f.as_str()).collect::<Array>()),
            }
        }
        if changed(&|o| o.action == self.action) {
            t["action"] = value(self.action.as_str());
        }
        if changed(&|o| o.label == self.label) {
            match &self.label {
                Some(l) => t["label"] = value(l.as_str()),
                None => {
                    t.remove("label");
                }
            }
        }
        for (key, now, before) in [("sites", &self.sites, old.map(|o| &o.sites)), ("boards", &self.boards, old.map(|o| &o.boards))] {
            if before != Some(now) {
                if now.is_empty() {
                    t.remove(key);
                } else {
                    t[key] = list(now);
                }
            }
        }
        if changed(&|o| o.enabled == self.enabled) {
            if self.enabled {
                t.remove("enabled");
            } else {
                t["enabled"] = value(false);
            }
        }
    }
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Mark {
    /// Hidden: by the filter with this label, or by hand (`""`).
    pub hidden: Option<String>,
    pub highlight: Option<String>,
}

#[derive(Default)]
pub struct Filters(Vec<Filter>);

impl Filters {
    /// The enabled filters (every one is checked, enabled or not).
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
            if !c.enabled {
                continue;
            }
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

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
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
pub mod tests {
    use ratatui::text::Line;

    use super::*;
    use crate::model::Attachment;

    /// Filters from `[[filter]]` tables.
    pub fn filters(toml_text: &str) -> Result<Filters> {
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
    fn disabled_filters_are_kept_but_not_applied() {
        let f = filters("[[filter]]\npattern = \"crypto\"\nenabled = false\n[[filter]]\npattern = \"x\"").unwrap();
        assert_eq!(f.len(), 1);
        assert!(f.check("4chan", "g", &post("crypto", "")).hidden.is_none());
        // A disabled filter must still be a valid one.
        assert!(filters("[[filter]]\npattern = \"(\"\nenabled = false").is_err());
    }

    #[test]
    fn written_tables_read_back() {
        let mut every = FilterConfig::new("a.b".into(), &[Field::Name, Field::Md5]);
        every.action = FilterAction::Highlight;
        every.label = Some("lbl".into());
        every.sites = vec!["4chan".into()];
        every.boards = vec!["g".into(), "v".into()];
        every.enabled = false;
        for c in [FilterConfig::new("p".into(), &[Field::Subject, Field::Comment]), FilterConfig::new("q".into(), &[Field::Filename]), every.clone()] {
            let mut t = toml_edit::Table::new();
            c.write(&mut t, None);
            let back: FilterConfig = toml::from_str(&toml_edit::DocumentMut::from(t.clone()).to_string()).unwrap();
            assert_eq!(back, c);
            // Changed back to the plain one: only what differs is touched, and it reads back.
            let plain = FilterConfig::new("p".into(), &[Field::Subject, Field::Comment]);
            plain.write(&mut t, Some(&c));
            let back: FilterConfig = toml::from_str(&toml_edit::DocumentMut::from(t).to_string()).unwrap();
            assert_eq!(back, plain);
        }
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
