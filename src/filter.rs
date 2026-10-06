//! `[[filter]]` rules: hide or highlight threads and posts by regex (or file MD5).

use std::collections::{HashMap, VecDeque};

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
    /// The poster ID.
    Id,
    /// The flag's code, or its name (`US`, `United States`).
    Flag,
    Tripcode,
    Capcode,
    /// A file's width and height, as `1920x1080`.
    Dimensions,
    /// A file's size against a range (not a regex): `>2MB`, `<=100KB`, `1MB-5MB`.
    Filesize,
    /// The post's number, in digits.
    Postno,
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
    /// What it hides in a thread, its replies are hidden with (and theirs), as
    /// `recursive_hiding` does for every hidden post.
    #[serde(default)]
    pub recursive: bool,
    /// Only OPs (threads), or only replies.
    #[serde(default)]
    pub op: bool,
    #[serde(default)]
    pub reply: bool,
    /// A desktop notification when a watched thread's refresh (or a followed general's
    /// board) brings a post it catches.
    #[serde(default)]
    pub notify: bool,
    /// What it highlights comes first in catalogs, whatever the sort.
    #[serde(default)]
    pub top: bool,
}

fn yes() -> bool {
    true
}

impl Field {
    pub const ALL: [Field; 12] = [
        Field::Subject,
        Field::Comment,
        Field::Name,
        Field::Filename,
        Field::Md5,
        Field::Id,
        Field::Flag,
        Field::Tripcode,
        Field::Capcode,
        Field::Dimensions,
        Field::Filesize,
        Field::Postno,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Field::Subject => "subject",
            Field::Comment => "comment",
            Field::Name => "name",
            Field::Filename => "filename",
            Field::Md5 => "md5",
            Field::Id => "id",
            Field::Flag => "flag",
            Field::Tripcode => "tripcode",
            Field::Capcode => "capcode",
            Field::Dimensions => "dimensions",
            Field::Filesize => "filesize",
            Field::Postno => "postno",
        }
    }

    /// Whether `pattern` is a regex for it (not an MD5 or a size range).
    fn is_regex(self) -> bool {
        !matches!(self, Field::Md5 | Field::Filesize)
    }
}

/// A `filesize` pattern as the sizes it takes in, bytes, both ends included: `>2MB`,
/// `>=2MB`, `<100KB`, `<=100KB`, `1MB-5MB`, or one size exactly. Units B, KB, MB, GB (of
/// 1024), any case, `K`/`M`/`G` too, bytes when left out.
pub fn size_range(pattern: &str) -> Option<(u64, u64)> {
    let size = |s: &str| -> Option<u64> {
        let s = s.trim().to_ascii_lowercase();
        let digits = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
        let (n, unit) = s.split_at(digits);
        let n: f64 = n.parse().ok().filter(|n: &f64| n.is_finite())?;
        let unit = match unit.trim() {
            "" | "b" => 1u64,
            "k" | "kb" | "kib" => 1 << 10,
            "m" | "mb" | "mib" => 1 << 20,
            "g" | "gb" | "gib" => 1 << 30,
            _ => return None,
        };
        // (A float past u64 saturates.)
        Some((n * unit as f64).round() as u64)
    };
    let p = pattern.trim();
    if let Some(s) = p.strip_prefix(">=") {
        Some((size(s)?, u64::MAX))
    } else if let Some(s) = p.strip_prefix("<=") {
        Some((0, size(s)?))
    } else if let Some(s) = p.strip_prefix('>') {
        Some((size(s)?.saturating_add(1), u64::MAX))
    } else if let Some(s) = p.strip_prefix('<') {
        Some((0, size(s)?.checked_sub(1)?))
    } else if let Some((a, b)) = p.split_once('-') {
        Some((size(a)?, size(b)?)).filter(|(a, b)| a <= b)
    } else {
        size(p).map(|s| (s, s))
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
        let mut c = FilterConfig {
            pattern,
            field: None,
            sites: Vec::new(),
            boards: Vec::new(),
            action: FilterAction::Hide,
            label: None,
            enabled: true,
            recursive: false,
            op: false,
            reply: false,
            notify: false,
            top: false,
        };
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
        let flags = [
            ("enabled", self.enabled, old.map(|o| o.enabled), true),
            ("recursive", self.recursive, old.map(|o| o.recursive), false),
            ("op", self.op, old.map(|o| o.op), false),
            ("reply", self.reply, old.map(|o| o.reply), false),
            ("notify", self.notify, old.map(|o| o.notify), false),
            ("top", self.top, old.map(|o| o.top), false),
        ];
        for (key, now, before, default) in flags {
            if before != Some(now) {
                if now == default {
                    t.remove(key);
                } else {
                    t[key] = value(now);
                }
            }
        }
    }
}

struct Filter {
    re: Option<Regex>,
    /// For `filesize`.
    sizes: Option<(u64, u64)>,
    pattern: String,
    fields: Vec<Field>,
    sites: Vec<String>,
    boards: Vec<String>,
    action: FilterAction,
    label: String,
    recursive: bool,
    op: bool,
    reply: bool,
    notify: bool,
    top: bool,
}

/// What filters (and manual hiding) say about a thread or post.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Mark {
    pub hidden: Option<Hidden>,
    pub highlight: Option<String>,
    /// Its replies are hidden with it (a `recursive` filter hides it).
    pub recursive: bool,
    /// The label of a `notify` filter that catches it.
    pub notify: Option<String>,
    /// A `top` filter highlights it: first in catalogs.
    pub top: bool,
}

/// Why a thread or post is hidden.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Hidden {
    ByHand,
    /// By the filter (or hidden word) with this label.
    ByFilter(String),
    /// It replies to this hidden post (recursive hiding).
    Reply(u64),
}

impl Hidden {
    /// The label of the filter that hides it, if one does.
    pub fn filter(&self) -> Option<&str> {
        if let Self::ByFilter(label) = self { Some(label) } else { None }
    }
}

/// Hidden words: one case-insensitive pattern, a group per word (to say which caught a post).
struct Words {
    re: Regex,
    words: Vec<String>,
}

/// A hidden word as a pattern: as typed, any case, as a whole word where it starts or ends
/// with a letter or digit (`cat`, not "concatenate"; `c++` as it is), any spaces between
/// the words of a phrase. `None` for nothing but spaces.
pub fn word_pattern(word: &str) -> Option<String> {
    let pieces: Vec<String> = word.split_whitespace().map(regex::escape).collect();
    let (first, last) = (word.trim().chars().next()?, word.trim().chars().last()?);
    let edge = |c: char| if c.is_alphanumeric() || c == '_' { r"\b" } else { "" };
    Some(format!("{}{}{}", edge(first), pieces.join(r"\s+"), edge(last)))
}

#[derive(Default)]
pub struct Filters(Vec<Filter>, Option<Words>);

impl Filters {
    /// The config's filters and hidden words.
    pub fn from_config(cfgs: &[FilterConfig], hidden_words: &[String]) -> Result<Self> {
        Self::new(cfgs)?.with_words(hidden_words)
    }

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
            let re = if fields.iter().any(|f| f.is_regex()) {
                Some(Regex::new(&c.pattern).with_context(|| format!("[[filter]] #{} (pattern = {:?})", i + 1, c.pattern))?)
            } else {
                None
            };
            let sizes = match fields.contains(&Field::Filesize) {
                true => Some(size_range(&c.pattern).with_context(|| {
                    format!("[[filter]] #{}: {:?} isn't a file size range (try \">2MB\", \"<100KB\", \"1MB-5MB\")", i + 1, c.pattern)
                })?),
                false => None,
            };
            if c.op && c.reply {
                bail!("[[filter]] #{}: `op` and `reply` together catch nothing (leave both out for all posts)", i + 1);
            }
            if !c.enabled {
                continue;
            }
            out.push(Filter {
                re,
                sizes,
                pattern: c.pattern.clone(),
                fields,
                sites: c.sites.clone(),
                boards: c.boards.clone(),
                action: c.action,
                label: c.label.clone().unwrap_or_else(|| c.pattern.clone()),
                recursive: c.recursive,
                op: c.op,
                reply: c.reply,
                notify: c.notify,
                top: c.top,
            });
        }
        Ok(Self(out, None))
    }

    /// With `hidden_words` too: any post with one of them is hidden, everywhere.
    pub fn with_words(mut self, words: &[String]) -> Result<Self> {
        let words: Vec<String> = words.iter().filter(|w| !w.trim().is_empty()).cloned().collect();
        if words.is_empty() {
            self.1 = None;
            return Ok(self);
        }
        let groups: Vec<String> = words.iter().filter_map(|w| word_pattern(w)).map(|p| format!("({p})")).collect();
        let re = Regex::new(&format!("(?i){}", groups.join("|"))).context("hidden_words")?;
        self.1 = Some(Words { re, words });
        Ok(self)
    }

    /// What the filters say about a post on `site`'s `board` (an OP, or a reply).
    pub fn check(&self, site: &str, board: &str, p: &Post, is_op: bool) -> Mark {
        let mut mark = Mark::default();
        let mut hidden: Option<String> = None;
        let mut comment: Option<String> = None;
        for f in &self.0 {
            let slot = match f.action {
                FilterAction::Hide => &mut hidden,
                FilterAction::Highlight => &mut mark.highlight,
            };
            // A recursive filter still counts on a post another filter hid first, and so do
            // ones that notify or put it first.
            let spreads = f.action == FilterAction::Hide && f.recursive && !mark.recursive;
            let more = spreads || (f.notify && mark.notify.is_none()) || (f.top && f.action == FilterAction::Highlight && !mark.top);
            if (slot.is_some() && !more)
                || (f.op && !is_op)
                || (f.reply && is_op)
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
                    Field::Id => p.id.as_deref().is_some_and(re),
                    Field::Flag => p.flag.as_ref().is_some_and(|fl| (!fl.code.is_empty() && re(&fl.code)) || (!fl.name.is_empty() && re(&fl.name))),
                    Field::Tripcode => p.trip.as_deref().is_some_and(re),
                    Field::Capcode => p.capcode.as_deref().is_some_and(re),
                    Field::Dimensions => p.files.iter().any(|file| file.width.zip(file.height).is_some_and(|(w, h)| re(&format!("{w}x{h}")))),
                    Field::Filesize => p.files.iter().any(|file| file.size.zip(f.sizes).is_some_and(|(s, (lo, hi))| (lo..=hi).contains(&s))),
                    Field::Postno => re(&p.no.to_string()),
                }
            });
            if hit {
                slot.get_or_insert_with(|| f.label.clone());
                mark.recursive |= spreads;
                if f.notify {
                    mark.notify.get_or_insert_with(|| f.label.clone());
                }
                mark.top |= f.top && f.action == FilterAction::Highlight;
            }
        }
        if hidden.is_none()
            && let Some(w) = &self.1
        {
            let comment = comment.get_or_insert_with(|| {
                p.body.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n")
            });
            let texts = p.subject.iter().map(String::as_str).chain([p.name.as_str(), comment.as_str()]).chain(p.files.iter().map(|f| f.filename.as_str()));
            // Which word: the group that matched (asked only of a text that matches).
            for text in texts.filter(|t| w.re.is_match(t)) {
                if let Some(c) = w.re.captures(text)
                    && let Some(i) = (1..c.len()).find(|&i| c.get(i).is_some())
                {
                    hidden = Some(format!("hidden word: {}", w.words.get(i - 1).map_or("", String::as_str)));
                    break;
                }
            }
        }
        // A filter labelled "" (only by hand, in the config) has always passed for hiding by hand.
        mark.hidden = hidden.map(|label| if label.is_empty() { Hidden::ByHand } else { Hidden::ByFilter(label) });
        mark
    }
}

/// Recursive hiding: in a thread, the replies to a hidden post that `spreads` (posts that
/// quote it), and the replies to those, on down, are hidden too, as `Hidden::Reply` of the
/// hidden post they quote. A post hidden otherwise keeps its own reason. The OP is never
/// hidden this way, and never spreads it: everyone quotes it. Gone through once each, so
/// quote loops end, and without recursion, so a long chain doesn't overflow the stack.
pub fn spread_hiding(marks: &mut [Mark], posts: &[Post], index: &HashMap<u64, usize>, backlinks: &[Vec<u64>], spreads: impl Fn(&Mark) -> bool) {
    let mut seen: Vec<bool> = marks.iter().enumerate().map(|(i, m)| i > 0 && m.hidden.is_some() && spreads(m)).collect();
    let mut queue: VecDeque<usize> = seen.iter().enumerate().filter(|&(_, &s)| s).map(|(i, _)| i).collect();
    while let Some(i) = queue.pop_front() {
        let (Some(parent), Some(replies)) = (posts.get(i), backlinks.get(i)) else { continue };
        for no in replies {
            let Some(&j) = index.get(no).filter(|&&j| j != 0) else { continue };
            let (Some(s), Some(m)) = (seen.get_mut(j), marks.get_mut(j)) else { continue };
            if std::mem::replace(s, true) {
                continue;
            }
            m.hidden.get_or_insert(Hidden::Reply(parent.no));
            queue.push_back(j);
        }
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
        assert_eq!(f.0.len(), 4);
        // Subject, case-insensitive by the pattern's own flag.
        let m = f.check("lainchan", "b", &post("CRYPTO general", "hi"), false);
        assert_eq!(m.hidden, Some(Hidden::ByFilter("crypto".into())));
        // Comments match line by line with (?m)-less anchors over the whole text; the first
        // highlight wins.
        let m = f.check("lainchan", "g", &post("", ">be me"), false);
        assert_eq!((m.hidden, m.highlight.as_deref()), (None, Some("^>be me$")));
        // Other boards don't get the /g/-only filter; files and names are checked.
        let m = f.check("lainchan", "b", &post("", ">be me"), false);
        assert_eq!(m.highlight.as_deref(), Some("png"));
        // MD5s compare exactly, on the listed sites only.
        assert_eq!(f.check("4chan", "b", &post("", "x"), false).hidden, Some(Hidden::ByFilter("u8Vh17KxaDvUJ6bBcmE/eg==".into())));
    }

    #[test]
    fn ids_flags_trips_capcodes_files_and_numbers() {
        use crate::model::Flag;
        let p = Post {
            no: 487211260,
            id: Some("Ab3dEf+g".into()),
            flag: Some(Flag { code: "PL".into(), name: "Poland".into() }),
            trip: Some("!!Fz3mQwerty".into()),
            capcode: Some("mod".into()),
            files: vec![Attachment { width: Some(1920), height: Some(1080), size: Some(3 << 20), ..Default::default() }],
            ..post("", "")
        };
        let catches = |field: &str, pattern: &str| {
            let f = filters(&format!("[[filter]]\npattern = {pattern:?}\nfield = {field:?}")).unwrap();
            f.check("4chan", "pol", &p, false).hidden.is_some()
        };
        assert!(catches("id", "^Ab3dEf\\+g$") && !catches("id", "^Zq9"));
        // A flag by its code or its name.
        assert!(catches("flag", "^PL$") && catches("flag", "(?i)^poland$") && !catches("flag", "^US$"));
        assert!(catches("tripcode", "^!!Fz3m") && !catches("tripcode", "Kot"));
        assert!(catches("capcode", "^mod$") && !catches("capcode", "admin"));
        assert!(catches("dimensions", "^1920x1080$") && !catches("dimensions", "^1080x"));
        assert!(catches("filesize", ">2MB") && catches("filesize", "1MB-4MB") && !catches("filesize", "<3mb") && !catches("filesize", ">3MB"));
        assert!(catches("filesize", ">=3MB") && catches("filesize", "<=3.0 mb") && catches("filesize", "3145728"));
        assert!(catches("postno", "260$") && catches("postno", "^4872") && !catches("postno", "^1"));
        // A post without them isn't caught by them.
        let plain = post("", "");
        for field in ["id", "flag", "tripcode", "capcode", "dimensions"] {
            let f = filters(&format!("[[filter]]\npattern = \".\"\nfield = {field:?}")).unwrap();
            assert!(f.check("4chan", "g", &plain, false).hidden.is_none(), "{field}");
        }
        // A size range that isn't one is reported.
        let err = filters("[[filter]]\npattern = \"big\"\nfield = \"filesize\"").err().unwrap();
        assert!(format!("{err:#}").contains("isn't a file size range"), "{err:#}");
    }

    #[test]
    fn size_ranges() {
        assert_eq!(size_range(">2MB"), Some((2 * 1024 * 1024 + 1, u64::MAX)));
        assert_eq!(size_range(">= 2 mb"), Some((2 << 20, u64::MAX)));
        assert_eq!(size_range("<100KB"), Some((0, 100 * 1024 - 1)));
        assert_eq!(size_range("<=1.5k"), Some((0, 1536)));
        assert_eq!(size_range("1MB-2GB"), Some((1 << 20, 2 << 30)));
        assert_eq!(size_range("500"), Some((500, 500)));
        for bad in ["", ">", "<0", "2MB-1MB", "big", "1TB", ">-1", "1e3", "NaN", "--1"] {
            assert_eq!(size_range(bad), None, "{bad}");
        }
        // Huge numbers saturate.
        assert_eq!(size_range(&format!(">={}GB", "9".repeat(40))), Some((u64::MAX, u64::MAX)));
    }

    #[test]
    fn ops_replies_notify_and_top() {
        let f = filters(
            "[[filter]]\npattern = \"x\"\nop = true\nlabel = \"ops\"\n\
             [[filter]]\npattern = \"y\"\nreply = true\nlabel = \"replies\"\n\
             [[filter]]\npattern = \"x|y\"\naction = \"highlight\"\nnotify = true\ntop = true\nlabel = \"watch\"",
        )
        .unwrap();
        let hidden = |p: &Post, op: bool| f.check("4chan", "g", p, op).hidden.and_then(|h| h.filter().map(String::from));
        assert_eq!((hidden(&post("x", ""), true), hidden(&post("x", ""), false)), (Some("ops".into()), None));
        assert_eq!((hidden(&post("y", ""), true), hidden(&post("y", ""), false)), (None, Some("replies".into())));
        // Notify and top still count when another filter got there first.
        let m = f.check("4chan", "g", &post("x", ""), true);
        assert_eq!((m.highlight.as_deref(), m.notify.as_deref(), m.top), (Some("watch"), Some("watch"), true));
        let f = filters("[[filter]]\npattern = \"x\"\naction = \"highlight\"\n[[filter]]\npattern = \"x\"\nnotify = true\ntop = true\nlabel = \"n\"").unwrap();
        let m = f.check("4chan", "g", &post("x", ""), false);
        // Top is for what it highlights; this one hides.
        assert_eq!((m.highlight.as_deref(), m.notify.as_deref(), m.top, m.hidden.is_some()), (Some("x"), Some("n"), false, true));
        assert!(f.check("4chan", "g", &post("z", ""), false).notify.is_none());
        // Both catch nothing: that's a mistake, said so.
        let err = filters("[[filter]]\npattern = \"x\"\nop = true\nreply = true").err().unwrap();
        assert!(format!("{err:#}").contains("`op` and `reply` together"), "{err:#}");
        assert!(filters("[[filter]]\npattern = \"x\"\nnotfy = true").is_err());
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
        assert_eq!(f.0.len(), 1);
        assert!(f.check("4chan", "g", &post("crypto", ""), false).hidden.is_none());
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
        every.recursive = true;
        every.op = true;
        every.notify = true;
        every.top = true;
        for c in [FilterConfig::new("p".into(), &[Field::Subject, Field::Comment]), FilterConfig::new("q".into(), &[Field::Filename]), every] {
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

    /// Posts numbered from 1, each quoting the posts listed for it; their index and backlinks.
    fn quoting(quotes: &[&[u64]]) -> (Vec<Post>, HashMap<u64, usize>, Vec<Vec<u64>>) {
        let posts: Vec<Post> = quotes.iter().enumerate().map(|(i, q)| Post { no: i as u64 + 1, quotes: q.to_vec(), ..Default::default() }).collect();
        let index: HashMap<u64, usize> = posts.iter().enumerate().map(|(i, p)| (p.no, i)).collect();
        let mut backlinks = vec![Vec::new(); posts.len()];
        for p in &posts {
            for q in &p.quotes {
                if let Some(&i) = index.get(q) {
                    backlinks[i].push(p.no);
                }
            }
        }
        (posts, index, backlinks)
    }

    fn hidden_by(marks: &[Mark]) -> Vec<Option<Hidden>> {
        marks.iter().map(|m| m.hidden.clone()).collect()
    }

    #[test]
    fn hiding_spreads_to_replies_and_theirs() {
        // 2 is hidden; 3 replies to it, 4 to 3, 5 to the OP only; 6 replies to 5 and 4.
        let (posts, index, backlinks) = quoting(&[&[], &[1], &[2], &[3, 1], &[1], &[5, 4]]);
        let mut marks = vec![Mark::default(); posts.len()];
        marks[1].hidden = Some(Hidden::ByHand);
        // The OP's own replies aren't caught by it being hidden.
        marks[0].hidden = Some(Hidden::ByHand);
        let before = marks.clone();
        spread_hiding(&mut marks, &posts, &index, &backlinks, |_| false);
        assert_eq!(marks, before);
        spread_hiding(&mut marks, &posts, &index, &backlinks, |_| true);
        let r = |no| Some(Hidden::Reply(no));
        assert_eq!(hidden_by(&marks), [Some(Hidden::ByHand), Some(Hidden::ByHand), r(2), r(3), None, r(4)]);
        // A post hidden for its own reason keeps it, and passes it on.
        let mut marks = vec![Mark::default(); posts.len()];
        marks[1].hidden = Some(Hidden::ByHand);
        marks[2].hidden = Some(Hidden::ByFilter("x".into()));
        spread_hiding(&mut marks, &posts, &index, &backlinks, |m| m.hidden == Some(Hidden::ByHand));
        assert_eq!(hidden_by(&marks), [None, Some(Hidden::ByHand), Some(Hidden::ByFilter("x".into())), r(3), None, r(4)]);
    }

    #[test]
    fn spreading_ends_in_quote_loops_and_long_chains() {
        // 2 and 3 quote each other.
        let (posts, index, backlinks) = quoting(&[&[], &[3], &[2], &[3]]);
        let mut marks = vec![Mark::default(); posts.len()];
        marks[1].hidden = Some(Hidden::ByHand);
        spread_hiding(&mut marks, &posts, &index, &backlinks, |_| true);
        assert_eq!(hidden_by(&marks), [None, Some(Hidden::ByHand), Some(Hidden::Reply(2)), Some(Hidden::Reply(3))]);
        // Each post replying to the one before, far deeper than a stack would go.
        let chain: Vec<Vec<u64>> = (0..200_000u64).map(|i| if i == 0 { vec![] } else { vec![i] }).collect();
        let refs: Vec<&[u64]> = chain.iter().map(Vec::as_slice).collect();
        let (posts, index, backlinks) = quoting(&refs);
        let mut marks = vec![Mark::default(); posts.len()];
        marks[1].hidden = Some(Hidden::ByHand);
        spread_hiding(&mut marks, &posts, &index, &backlinks, |_| true);
        assert!(marks.iter().skip(1).all(|m| m.hidden.is_some()));
        assert_eq!(marks.last().unwrap().hidden, Some(Hidden::Reply(199_999)));
    }

    #[test]
    fn recursive_filters_say_so_even_after_another_hides() {
        let f = filters("[[filter]]\npattern = \"crypto\"\n[[filter]]\npattern = \"crypto|nft\"\nrecursive = true\n[[filter]]\npattern = \"x\"\naction = \"highlight\"\nrecursive = true").unwrap();
        let m = f.check("4chan", "g", &post("crypto", ""), false);
        assert_eq!((m.hidden, m.recursive), (Some(Hidden::ByFilter("crypto".into())), true));
        assert!(f.check("4chan", "g", &post("nft", ""), false).recursive);
        // Highlighting doesn't hide, so nothing spreads.
        let m = f.check("4chan", "g", &post("x", ""), false);
        assert!(m.highlight.is_some() && !m.recursive);
    }

    #[test]
    fn checking_is_fast() {
        let toml_text: String = (0..20).map(|i| format!("[[filter]]\npattern = \"(?i)word{i}|other{i}\"\n")).collect();
        let f = filters(&toml_text).unwrap();
        let text = "lorem ipsum dolor sit amet consectetur adipiscing elit ".repeat(20);
        let posts: Vec<Post> = (0..300).map(|i| post(&format!("thread {i}"), &text)).collect();
        let start = std::time::Instant::now();
        let hidden = posts.iter().filter(|p| f.check("4chan", "g", p, false).hidden.is_some()).count();
        let took = start.elapsed();
        eprintln!("20 filters over 300 posts: {took:?}");
        assert_eq!(hidden, 0);
        // Run once per load, not per frame. About 1.3ms in a release build; debug is far slower.
        let limit = if cfg!(debug_assertions) { 3000 } else { 50 };
        assert!(took < std::time::Duration::from_millis(limit), "{took:?}");
    }
}

#[cfg(test)]
mod word_tests {
    use ratatui::text::Line;

    use super::*;
    use crate::model::Attachment;

    fn words(w: &[&str]) -> Filters {
        Filters::new(&[]).unwrap().with_words(&w.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    fn says(body: &str) -> Post {
        Post { no: 1, name: "Anonymous".into(), body: body.lines().map(|l| Line::raw(l.to_string())).collect(), ..Default::default() }
    }

    fn hidden(f: &Filters, p: &Post) -> Option<String> {
        f.check("s", "b", p, false).hidden?.filter().map(String::from)
    }

    #[test]
    fn whole_words_any_case() {
        let f = words(&["cat", "c++", ":^)", "free money", "λ", "привет"]);
        let label = |w: &str| Some(format!("hidden word: {w}"));
        assert_eq!(hidden(&f, &says("Cat pics")), label("cat"));
        assert_eq!(hidden(&f, &says("concatenate the cats")), None);
        assert_eq!(hidden(&f, &says("I write C++ daily")), label("c++"));
        assert_eq!(hidden(&f, &says("nice :^)")), label(":^)"));
        assert_eq!(hidden(&f, &says("FREE\n   MONEY now")), label("free money"));
        assert_eq!(hidden(&f, &says("freemoney")), None);
        assert_eq!(hidden(&f, &says("the λ calculus")), label("λ"));
        assert_eq!(hidden(&f, &says("ПРИВЕТ всем")), label("привет"));
        assert_eq!(hidden(&f, &says("приветствую")), None);
        // Each field: subject, name, file names.
        let p = Post { subject: Some("about CAT".into()), ..says("x") };
        assert_eq!(hidden(&f, &p), label("cat"));
        let p = Post { name: "cat !trip".into(), ..says("x") };
        assert_eq!(hidden(&f, &p), label("cat"));
        let p = Post { files: vec![Attachment { filename: "my cat.png".into(), ..Default::default() }], ..says("x") };
        assert_eq!(hidden(&f, &p), label("cat"));
        // A filter that hides it says first.
        let both = Filters::new(&[FilterConfig::new("pics".into(), &[Field::Comment])]).unwrap().with_words(&["cat".into()]).unwrap();
        assert_eq!(hidden(&both, &says("cat pics")), Some("pics".into()));
        // Blank words are nothing.
        assert!(word_pattern("   ").is_none());
        assert_eq!(hidden(&words(&["  "]), &says("anything")), None);
    }

    /// Against a brute-force matcher: a word is found where its letters are, any case, with
    /// no letter or digit right before (after) it when it starts (ends) with one.
    #[test]
    fn random_posts_against_brute_force() {
        use crate::fuzz::Rng;
        let vocab = ["cat", "Cat", "concat", "c++", "rust", "RUST", "trust", "λ", "λx", "a b", "a  b", "ab", ":^)", "x", "über", "Über"];
        let brute = |text: &str, word: &str| -> bool {
            let pieces: Vec<String> = word.split_whitespace().map(str::to_lowercase).collect();
            let lower: Vec<char> = text.to_lowercase().chars().collect();
            let wordy = |c: char| c.is_alphanumeric() || c == '_';
            let (first, last) = (word.trim().chars().next().unwrap(), word.trim().chars().last().unwrap());
            (0..lower.len()).any(|start| {
                // Match the pieces with runs of spaces between them.
                let mut at = start;
                for (k, piece) in pieces.iter().enumerate() {
                    if k > 0 {
                        let ws = lower[at..].iter().take_while(|c| c.is_whitespace()).count();
                        if ws == 0 {
                            return false;
                        }
                        at += ws;
                    }
                    let pc: Vec<char> = piece.chars().collect();
                    if lower.len() < at + pc.len() || lower[at..at + pc.len()] != pc[..] {
                        return false;
                    }
                    at += pc.len();
                }
                let before_ok = !wordy(first) || start == 0 || !wordy(lower[start - 1]);
                let after_ok = !wordy(last) || at == lower.len() || !wordy(lower[at]);
                before_ok && after_ok
            })
        };
        for seed in 0..300 {
            let mut rng = Rng::new(seed);
            let text: String = (0..1 + rng.below(8)).map(|_| *rng.pick(&vocab)).collect::<Vec<_>>().join(*rng.pick(&[" ", "", ", ", "\n"]));
            let word = *rng.pick(&vocab);
            let f = words(&[word]);
            assert_eq!(hidden(&f, &says(&text)).is_some(), brute(&text, word), "seed {seed}: {word:?} in {text:?}");
        }
    }
}
