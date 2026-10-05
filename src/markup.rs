//! Turning post comments into styled ratatui lines, and wrapping them.
#![deny(clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]

use std::collections::HashSet;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::model::{Anchor, Link, Target};
use crate::theme::mark;

// Parsed posts carry marker colors, mapped to the current theme when drawn (`theme::paint`).
const PINKTEXT: Style = Style::new().fg(mark::PINKTEXT);
const SPOILER: Style = Style::new().fg(mark::SPOILER).bg(mark::SPOILER);

pub const GREENTEXT: Style = Style::new().fg(mark::GREENTEXT);
const QUOTELINK: Style = Style::new().fg(mark::QUOTELINK).add_modifier(Modifier::UNDERLINED);
/// A web link (not a quote link).
const LINK: Style = Style::new().fg(mark::LINK).add_modifier(Modifier::UNDERLINED);
const HEADING: Style = Style::new().fg(mark::HEADING).add_modifier(Modifier::BOLD);
const CODE: Style = Style::new().fg(mark::CODE);
/// Line style marking a line of a code block: `wrap` keeps its whitespace and never word-wraps it.
pub const CODE_LINE: Style = Style::new().fg(mark::CODE);
const CONTINUATION: &str = "↪";
/// Comments are cut off past this many bytes of markup (no imageboard allows posts near it;
/// a longer one is broken or hostile), and tags nested deeper than this are ignored.
const MAX_COMMENT: usize = 64 * 1024;
const MAX_DEPTH: usize = 64;
const CUT_OFF: &str = " [comment cut off: too long]";

/// Parsed comment: styled lines plus what it links to.
pub struct Parsed {
    pub lines: Vec<Line<'static>>,
    /// Post numbers quoted with a bare `>>123`, for in-thread jumps and backlinks.
    pub quotes: Vec<u64>,
    /// Every quote link with as much of its target as the markup gives.
    pub links: Vec<Link>,
    /// Web links: link targets and bare `http(s)://` URLs in the text.
    pub urls: Vec<String>,
    /// Every link, where it is in `lines`.
    pub anchors: Vec<Anchor>,
}

impl From<Parsed> for crate::model::Post {
    /// A post with this body (and its quotes and links), the rest left to fill in.
    fn from(p: Parsed) -> Self {
        Self { body: p.lines, quotes: p.quotes, links: p.links, urls: p.urls, anchors: p.anchors, ..Default::default() }
    }
}

/// The HTML dialects differ in a few ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// 4chan: `<s>` is a spoiler.
    Fourchan,
    /// vichan: `<s>` is strikethrough, spoilers are `span.spoiler`.
    Vichan,
    /// LynxChan's `markdown` field: raw newlines are line breaks.
    Lynxchan,
    /// jschan: raw newlines are line breaks, and `<small>(OP)</small>` annotations are dropped.
    Jschan,
    /// 2ch.hk's makaba: quote links carry their own " (OP)", which is dropped.
    Makaba,
}

/// An open tag while parsing.
struct Open {
    name: String,
    style: Style,
    href: Option<String>,
    /// `<pre>` or a highlighted code `<div>`.
    code: bool,
}

/// Parse the comment HTML used by 4chan, vichan and LynxChan.
pub fn parse_html(html: &str, flavor: Flavor) -> Parsed {
    let mut b = Builder::default();
    let mut stack: Vec<Open> = Vec::new();
    let (html, cut) = cut_off(html);
    let mut rest = html;

    while !rest.is_empty() {
        let (text, tag_and_after) = match rest.split_once('<') {
            Some((text, after)) => (text, Some(after)),
            None => (rest, None),
        };
        if !text.is_empty() && !stack.iter().any(|o| o.name == "small" && flavor == Flavor::Jschan) {
            let text = decode(text);
            let style = stack.last().map(|o| o.style).unwrap_or_default();
            if stack.iter().any(|o| o.code) {
                b.code_text(&text, style);
            } else {
                let text = text.replace('\r', "");
                let newlines = matches!(flavor, Flavor::Lynxchan | Flavor::Jschan);
                let text = if newlines { text } else { text.replace('\n', " ") };
                let href = stack.iter().rev().find_map(|o| o.href.as_deref());
                for (i, part) in text.split('\n').enumerate() {
                    if i > 0 {
                        b.newline();
                    }
                    let part = match (flavor, href) {
                        (Flavor::Makaba, Some(_)) => part.strip_suffix(" (OP)").unwrap_or(part),
                        _ => part,
                    };
                    b.text(part, style, href);
                }
            }
        }
        let Some(after) = tag_and_after else { break };
        let Some((tag, after_tag)) = after.split_once('>') else {
            // Unterminated tag: treat the remainder as text.
            b.text(&decode(&format!("<{after}")), stack.last().map(|o| o.style).unwrap_or_default(), None);
            break;
        };
        rest = after_tag;

        let closing = tag.starts_with('/');
        let tag_body = tag.trim_start_matches('/').trim_end_matches('/');
        let name = tag_body.split(|c: char| c.is_whitespace()).next().unwrap_or("").to_ascii_lowercase();
        match name.as_str() {
            "br" => b.newline(),
            "wbr" | "img" => {}
            _ if closing => {
                if let Some(i) = stack.iter().rposition(|o| o.name == name) {
                    let open = stack.split_off(i).swap_remove(0);
                    if open.code {
                        b.end_line();
                    } else if matches!(open.name.as_str(), "p" | "div") {
                        b.newline();
                    }
                }
            }
            _ => {
                let class = attr(tag_body, "class").unwrap_or_default().to_ascii_lowercase();
                let block = name == "pre" || (name == "div" && class.contains("hljs"));
                let base = stack.last().map(|o| o.style).unwrap_or_default();
                let href = if name == "a" { attr(tag_body, "href").map(|h| decode(&h)) } else { None };
                // Links to other sites, as opposed to quote links (which have a class saying so).
                let web = href.as_deref().filter(|h| {
                    (h.starts_with("http://") || h.starts_with("https://"))
                        && !["quote", "backlink", "reply"].iter().any(|c| class.contains(c))
                });
                if let Some(h) = web {
                    b.url(h);
                }
                let style = match name.as_str() {
                    _ if block => base.patch(CODE),
                    "a" if web.is_some() => base.patch(LINK),
                    "a" => base.patch(QUOTELINK),
                    "b" | "strong" => base.add_modifier(Modifier::BOLD),
                    "i" | "em" => base.add_modifier(Modifier::ITALIC),
                    "u" => base.add_modifier(Modifier::UNDERLINED),
                    "s" if flavor == Flavor::Fourchan => base.patch(SPOILER),
                    "s" | "del" | "strike" => base.add_modifier(Modifier::CROSSED_OUT),
                    "code" => base.patch(CODE),
                    "span" | "div" | "p" => match class.as_str() {
                        c if c.contains("spoiler") => base.patch(SPOILER),
                        c if c.contains("quote") || c.contains("greentext") || c == "unkfunc" => base.patch(GREENTEXT),
                        c if c.contains("heading") || c.contains("redtext") || c == "title" => base.patch(HEADING),
                        c if c.contains("pinktext") || c.contains("orangetext") => base.patch(PINKTEXT),
                        // jschan's inline formatting.
                        "bold" => base.add_modifier(Modifier::BOLD),
                        "em" => base.add_modifier(Modifier::ITALIC),
                        "underline" | "u" => base.add_modifier(Modifier::UNDERLINED),
                        "strike" | "s" => base.add_modifier(Modifier::CROSSED_OUT),
                        "mono" => base.patch(CODE),
                        _ => base,
                    },
                    _ => base,
                };
                if stack.len() >= MAX_DEPTH {
                    continue;
                }
                if block {
                    b.end_line();
                }
                stack.push(Open { name, style, href, code: block });
            }
        }
    }
    if cut {
        b.text(CUT_OFF, Style::new(), None);
    }
    b.finish()
}

/// `s` up to `MAX_COMMENT` bytes (on a character boundary), and whether it was cut.
fn cut_off(s: &str) -> (&str, bool) {
    if s.len() <= MAX_COMMENT {
        return (s, false);
    }
    let end = (0..=MAX_COMMENT).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
    (s.get(..end).unwrap_or_default(), true)
}

/// Parse LynxChan's plain-text `message` field (markup is in-band). Used when a post has no
/// rendered `markdown`.
pub fn parse_plain(text: &str) -> Parsed {
    let mut b = Builder::default();
    let (text, cut) = cut_off(text);
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            b.newline();
        }
        let style = if line.starts_with('>') && !line.starts_with(">>") {
            GREENTEXT
        } else if line.starts_with('<') {
            PINKTEXT
        } else if line.starts_with("==") && line.ends_with("==") && line.len() > 4 {
            HEADING
        } else {
            Style::new()
        };
        b.text(line, style, None);
    }
    if cut {
        b.text(CUT_OFF, Style::new(), None);
    }
    b.finish()
}

#[derive(Default)]
struct Builder {
    lines: Vec<Line<'static>>,
    cur: Vec<Span<'static>>,
    /// The bytes in `cur`.
    cur_len: usize,
    /// The current line is part of a code block.
    cur_code: bool,
    /// Don't merge the next text into the last span (it's a quote link).
    sealed: bool,
    quotes: Vec<u64>,
    links: Vec<Link>,
    urls: Vec<String>,
    anchors: Vec<Anchor>,
    /// What's in `quotes`, `links` and `urls`, to add each once without searching them.
    seen_quotes: HashSet<u64>,
    seen_links: HashSet<Link>,
    seen_urls: HashSet<String>,
}

impl Builder {
    /// Where the next text goes: the line, and the byte offset in it.
    fn at(&self) -> (usize, usize) {
        (self.lines.len(), self.cur_len)
    }

    /// A web link, if it's new.
    fn url(&mut self, url: &str) {
        if !self.seen_urls.contains(url) {
            self.seen_urls.insert(url.to_string());
            self.urls.push(url.to_string());
        }
    }

    /// Note a link over the text just pushed from `start` (joined to the previous piece of
    /// the same link, when it's right before it on the line).
    fn anchor(&mut self, (line, start): (usize, usize), to: Target) {
        let end = self.at().1;
        if end == start {
            return;
        }
        match self.anchors.last_mut() {
            Some(a) if a.line == line && a.end == start && a.to == to => a.end = end,
            _ => self.anchors.push(Anchor { line, start, end, to }),
        }
    }

    /// Add text, turning `>>123`, `>>>/b/123` and `>>>/b/` into quote links, each its own
    /// span. `href` is the enclosing `<a>`'s target, which knows the thread of cross-thread links.
    fn text(&mut self, text: &str, style: Style, href: Option<&str>) {
        // Text of a link to another site (a quote link's style is different).
        let web = href.filter(|_| style.fg == Some(mark::LINK)).map(|h| Target::Url(h.to_string()));
        let mut rest = text;
        while let Some((before, from)) = rest.find(">>").and_then(|pos| rest.split_at_checked(pos)) {
            let at = self.at();
            self.push(before, style);
            if let Some(to) = &web {
                self.anchor(at, to.clone());
            }
            let Some(((quote, after), mut link)) = quote_at(from).and_then(|(len, link)| Some((from.split_at_checked(len)?, link)))
            else {
                // Not a quote link: the `>>` is text.
                let (arrows, after) = from.split_at_checked(2).unwrap_or((from, ""));
                self.push(arrows, style);
                rest = after;
                continue;
            };
            // The href is canonical (the text may use a board alias, like /lambda/ for /λ/).
            if let Some(h) = href.map(parse_href) {
                link.board = h.board.or(link.board);
                link.thread = h.thread;
                link.post = link.post.or(h.post);
            }
            // `>>>/b/123` names a board; only a bare `>>123` can be in this thread.
            if let Some(n) = link.post
                && !quote.starts_with(">>>")
                && self.seen_quotes.insert(n)
            {
                self.quotes.push(n);
            }
            if self.seen_links.insert(link.clone()) {
                self.links.push(link.clone());
            }
            let at = self.at();
            self.cur.push(Span::styled(quote.to_string(), style.patch(QUOTELINK)));
            self.cur_len = self.cur_len.saturating_add(quote.len());
            self.anchor(at, Target::Quote(link));
            self.sealed = true;
            rest = after;
        }
        let at = self.at();
        self.push(rest, style);
        if let Some(to) = web {
            self.anchor(at, to);
        }
    }

    /// Text inside a code block: whitespace kept, newlines are line breaks, no quote links.
    fn code_text(&mut self, text: &str, style: Style) {
        for (i, part) in text.replace('\r', "").split('\n').enumerate() {
            if i > 0 {
                self.newline();
            }
            self.cur_code = true;
            self.push(part, style);
        }
    }

    fn push(&mut self, text: &str, style: Style) {
        // Measured the same by every terminal, from here on (wrapping, links, drawing).
        let text = &*for_terminal(text);
        if text.is_empty() {
            return;
        }
        match self.cur.last_mut() {
            Some(last) if last.style == style && !self.sealed => last.content.to_mut().push_str(text),
            _ => self.cur.push(Span::styled(text.to_string(), style)),
        }
        self.cur_len = self.cur_len.saturating_add(text.len());
        self.sealed = false;
    }

    fn newline(&mut self) {
        let mut line = Line::from(std::mem::take(&mut self.cur));
        self.cur_len = 0;
        if std::mem::take(&mut self.cur_code) {
            line.style = CODE_LINE;
        }
        self.lines.push(line);
        self.sealed = false;
    }

    /// Finish the current line if it has anything on it (code blocks are block-level).
    fn end_line(&mut self) {
        if !self.cur.is_empty() {
            self.newline();
        }
        self.cur_code = false;
    }

    fn finish(mut self) -> Parsed {
        if !self.cur.is_empty() {
            self.newline();
        }
        // Drop trailing blank lines.
        while self.lines.last().is_some_and(|l| l.width() == 0) {
            self.lines.pop();
        }
        // Bare URLs: found per line, after <wbr> and the like have been joined up. The links
        // so far are in order and don't overlap, so each line's are found by bisecting.
        self.anchors.sort_by_key(|a| (a.line, a.start));
        let (mut found, mut bare) = (Vec::new(), Vec::new());
        for (n, line) in self.lines.iter_mut().enumerate().filter(|(_, l)| l.style != CODE_LINE) {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let ranges = find_urls(&text);
            let all = &self.anchors;
            let on_line = all.get(all.partition_point(|x| x.line < n)..all.partition_point(|x| x.line <= n)).unwrap_or_default();
            for &(a, b) in &ranges {
                let Some(url) = text.get(a..b) else { continue };
                found.push(url.to_string());
                // A link whose text is its URL is noted once.
                let overlaps = on_line.get(on_line.partition_point(|x| x.end <= a)).is_some_and(|x| x.start < b);
                if !overlaps {
                    bare.push(Anchor { line: n, start: a, end: b, to: Target::Url(url.to_string()) });
                }
            }
            if !ranges.is_empty() {
                line.spans = style_ranges(std::mem::take(&mut line.spans), &ranges);
            }
        }
        for url in found {
            self.url(&url);
        }
        self.anchors.extend(bare);
        // Lines dropped from the end take their links along.
        let lines = self.lines.len();
        self.anchors.retain(|a| a.line < lines);
        self.anchors.sort_by_key(|a| (a.line, a.start));
        Parsed { lines: self.lines, quotes: self.quotes, links: self.links, urls: self.urls, anchors: self.anchors }
    }
}

/// Byte ranges of `http(s)://` URLs in text. Trailing punctuation is left out, and so is a
/// closing bracket that has no opening one in the URL.
fn find_urls(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(tail) = s.get(from..)
        && let Some(i) = tail.find("http")
    {
        let start = from.saturating_add(i);
        let Some(here) = s.get(start..).filter(|u| u.starts_with("http://") || u.starts_with("https://")) else {
            from = start.saturating_add(4);
            continue;
        };
        let url_end = |c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`');
        let end = here.find(url_end).map_or(s.len(), |e| start.saturating_add(e));
        let mut url = s.get(start..end).unwrap_or_default();
        // Trailing punctuation, and closing brackets without an opening one, aren't part of it.
        // The brackets are counted once, and the counts kept as the end comes off.
        let count = |c| url.bytes().filter(|&b| b == c).count();
        let (mut parens, mut brackets) = ((count(b'('), count(b')')), (count(b'['), count(b']')));
        loop {
            if url.ends_with(['.', ',', ';', ':', '!', '?', '\'', '*']) {
            } else if url.ends_with(')') && parens.0 < parens.1 {
                parens.1 = parens.1.saturating_sub(1);
            } else if url.ends_with(']') && brackets.0 < brackets.1 {
                brackets.1 = brackets.1.saturating_sub(1);
            } else {
                break;
            }
            url = url.get(..url.len().saturating_sub(1)).unwrap_or_default();
        }
        let host = url.split_once("://").map_or("", |(_, h)| h);
        if !host.is_empty() && !host.starts_with('/') {
            out.push((start, start.saturating_add(url.len())));
        }
        from = end.max(start.saturating_add(1));
    }
    out
}

/// Give the parts of `spans` inside `ranges` (byte offsets into their joined text) the link
/// style, splitting spans as needed. Spoilers and quote links keep their style.
fn style_ranges(spans: Vec<Span<'static>>, ranges: &[(usize, usize)]) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut off = 0usize;
    // The ranges are in order and don't overlap: the first one not yet behind this span.
    let mut first = 0;
    for s in spans {
        let len = s.content.len();
        let span_end = off.saturating_add(len);
        let keep = is_spoiler(s.style) || is_quote_link(s.style) || s.style.fg == Some(mark::LINK);
        while ranges.get(first).is_some_and(|&(_, b)| b <= off) {
            first = first.saturating_add(1);
        }
        let near = ranges.get(first..).unwrap_or_default();
        let near = near.get(..near.partition_point(|&(a, _)| a < span_end)).unwrap_or_default();
        let mut cuts: Vec<usize> = near.iter().flat_map(|&(a, b)| [a, b]).filter(|&c| c > off && c < span_end).map(|c| c.saturating_sub(off)).collect();
        cuts.sort_unstable();
        cuts.dedup();
        let (mut at, mut k) = (0, 0);
        for end in cuts.into_iter().chain([len]) {
            let piece = s.content.get(at..end).unwrap_or_default();
            let pos = off.saturating_add(at);
            while near.get(k).is_some_and(|&(_, b)| b <= pos) {
                k = k.saturating_add(1);
            }
            let inside = near.get(k).is_some_and(|&(a, b)| pos >= a && pos < b);
            let style = if inside && !keep { s.style.patch(LINK) } else { s.style };
            out.push(Span::styled(piece.to_string(), style));
            at = end;
        }
        off = off.saturating_add(len);
    }
    out
}

/// Text as every terminal agrees on its width. Emoji sequences built from several code
/// points (joined with U+200D, with a skin tone, variation selector, keycap or tags, and
/// flags) are one glyph in some terminals and several in others. Where a terminal and the
/// layout disagree, the rest of the line lands in the wrong cells and what was drawn there
/// before shows through. So: a joined sequence's first emoji, no skin tones or selectors,
/// a flag as its two letters. No bidi controls either. Never wider than the original.
pub fn for_terminal(s: &str) -> std::borrow::Cow<'_, str> {
    let odd = |c: char| {
        matches!(c, '\u{200d}' | '\u{fe0e}' | '\u{fe0f}' | '\u{20e3}' | '\u{1f3fb}'..='\u{1f3ff}' | '\u{1f1e6}'..='\u{1f1ff}' | '\u{e0020}'..='\u{e007f}')
            // Bidi embeddings, overrides and isolates: they'd turn the rest of the text
            // around (a file name ending "gpj.exe" shown as "exe.jpg").
            || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    };
    if !s.chars().any(odd) {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            // What a joiner joins goes with it.
            '\u{200d}' => {
                chars.next();
            }
            #[allow(clippy::arithmetic_side_effects, clippy::cast_possible_truncation)] // c is in the range just matched: 0..26
            '\u{1f1e6}'..='\u{1f1ff}' => out.push(char::from(b'A' + (c as u32 - 0x1f1e6) as u8)),
            c if odd(c) => {}
            c => out.push(c),
        }
    }
    std::borrow::Cow::Owned(out)
}

/// `line` with each byte range in `ranges` restyled by `f` (given the range's index), its
/// spans split where needed.
pub fn restyle(line: &Line<'static>, ranges: &[(usize, usize)], f: impl Fn(Style, usize) -> Style) -> Line<'static> {
    let mut out = Vec::new();
    let mut off = 0usize;
    for s in &line.spans {
        let len = s.content.len();
        let span_end = off.saturating_add(len);
        let mut cuts: Vec<usize> = ranges.iter().flat_map(|&(a, b)| [a, b]).filter(|&c| c > off && c < span_end).map(|c| c.saturating_sub(off)).collect();
        cuts.sort_unstable();
        cuts.dedup();
        let mut at = 0;
        for end in cuts.into_iter().chain([len]) {
            let Some(piece) = s.content.get(at..end) else { continue };
            let pos = off.saturating_add(at);
            let style = match ranges.iter().position(|&(a, b)| pos >= a && pos < b) {
                Some(k) => f(s.style, k),
                None => s.style,
            };
            out.push(Span::styled(piece.to_string(), style));
            at = end;
        }
        off = off.saturating_add(len);
    }
    Line::from(out).style(line.style)
}

/// A quote link at the start of `s` (which starts with `>>`): its length and target.
fn quote_at(s: &str) -> Option<(usize, Link)> {
    if let Some(rest) = s.strip_prefix(">>>/") {
        // >>>/board/ or >>>/board/123
        let (board, after) = rest.split_once('/')?;
        if board.is_empty() || board.contains(|c: char| c.is_whitespace() || c == '>') {
            return None;
        }
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        let link = Link { board: Some(board.to_string()), thread: None, post: digits.parse().ok() };
        // `>>>/`, the board, `/`, the number.
        return Some((board.len().saturating_add(digits.len()).saturating_add(5), link));
    }
    let digits: String = s.strip_prefix(">>")?.chars().take_while(|c| c.is_ascii_digit()).collect();
    let post = digits.parse().ok()?;
    Some((digits.len().saturating_add(2), Link { board: None, thread: None, post: Some(post) }))
}

/// Read a quote link's target from its href: `#p123`, `/g/thread/123#p456`,
/// `//boards.4chan.org/g/`, `/b/res/123.html#456`, ... (any host is dropped).
fn parse_href(href: &str) -> Link {
    let (path, frag) = href.split_once('#').unwrap_or((href, ""));
    let path = match path.split_once("//") {
        Some((_, rest)) => rest.find('/').and_then(|i| rest.get(i..)).unwrap_or_default(),
        None => path,
    };
    let (board, thread, post) = crate::route::parse_path(path, frag);
    Link { board, thread, post }
}

/// The post number a quote-link span points at, if the whole span is `>>123`.
pub fn quote_target(span: &str) -> Option<u64> {
    span.strip_prefix(">>").filter(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))?.parse().ok()
}

pub fn is_quote_link(style: Style) -> bool {
    style.fg == Some(mark::QUOTELINK) && style.add_modifier.contains(Modifier::UNDERLINED)
}

pub fn is_spoiler(style: Style) -> bool {
    style.bg == SPOILER.bg && style.fg == SPOILER.fg
}

/// Spoilered text as it's shown until revealed: a shade in each cell it would take.
pub fn masked(text: &str) -> String {
    text.chars().map(|c| "░".repeat(unicode_width::UnicodeWidthChar::width(c).unwrap_or(0))).collect()
}

/// Show spoilered text in a line (still marked by its background).
pub fn reveal(line: &Line<'static>) -> Line<'static> {
    let mut l = line.clone();
    for s in &mut l.spans {
        if is_spoiler(s.style) {
            s.style = s.style.fg(mark::REVEALED);
        }
    }
    l
}

/// Highlight case-insensitive matches of `needle` (already lowercase) in a line. Hidden
/// spoilers aren't searched.
pub fn highlight(line: &Line<'static>, needle: &str, hl: Style) -> Line<'static> {
    if needle.is_empty() {
        return line.clone();
    }
    let mut spans = Vec::new();
    for s in &line.spans {
        let lower = s.content.to_lowercase();
        // Only when lowercasing keeps byte offsets (true for nearly all text).
        if is_spoiler(s.style) || lower.len() != s.content.len() || !lower.contains(needle) {
            spans.push(s.clone());
            continue;
        }
        // Lowercasing kept the offsets, so they're valid in the original too.
        let piece = |a: usize, b: usize| s.content.get(a..b).unwrap_or_default().to_string();
        let mut at = 0;
        for (i, m) in lower.match_indices(needle) {
            if i > at {
                spans.push(Span::styled(piece(at, i), s.style));
            }
            let end = i.saturating_add(m.len());
            spans.push(Span::styled(piece(i, end), s.style.patch(hl)));
            at = end;
        }
        if at < s.content.len() {
            spans.push(Span::styled(piece(at, s.content.len()), s.style));
        }
    }
    Line::from(spans).style(line.style)
}

fn attr(tag: &str, key: &str) -> Option<String> {
    let (_, rest) = tag.split_once(&format!("{key}="))?;
    let value = match rest.chars().next()? {
        q @ ('"' | '\'') => rest.strip_prefix(q)?.split(q).next()?,
        _ => rest.split(|c: char| c.is_whitespace()).next()?,
    };
    Some(value.to_string())
}

/// Decode HTML entities (also used for subjects and names).
pub fn decode(s: &str) -> String {
    html_escape::decode_html_entities(s).into_owned()
}

/// Word-wrap a styled line to `width` columns. Over-long words are split. Code lines keep
/// their whitespace and wrap by character, with a marker on each continuation line.
pub fn wrap(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(2);
    if line.style == CODE_LINE {
        return wrap_code(line, width);
    }
    let mut out = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0usize;

    for span in &line.spans {
        for token in split_keep_spaces(&span.content) {
            let tw = token.width();
            let is_space = token.starts_with(' ');
            if cur_w.saturating_add(tw) <= width {
                push_merged(&mut cur, token, span.style);
                cur_w = cur_w.saturating_add(tw);
            } else if is_space {
                // Break at whitespace; don't carry it to the next line.
                out.push(Line::from(std::mem::take(&mut cur)));
                cur_w = 0;
            } else if tw <= width {
                out.push(Line::from(std::mem::take(&mut cur)));
                push_merged(&mut cur, token, span.style);
                cur_w = tw;
            } else {
                for ch in token.chars() {
                    let cw = ch.to_string().width();
                    if cur_w.saturating_add(cw) > width {
                        out.push(Line::from(std::mem::take(&mut cur)));
                        cur_w = 0;
                    }
                    push_merged(&mut cur, &ch.to_string(), span.style);
                    cur_w = cur_w.saturating_add(cw);
                }
            }
        }
    }
    out.push(Line::from(cur));
    out
}

fn wrap_code(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    let marker = Style::new().fg(Color::DarkGray);
    let mut out = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0usize;
    for span in &line.spans {
        for ch in span.content.chars() {
            let mut buf = [0; 4];
            let ch = &*ch.encode_utf8(&mut buf);
            // Measured as a string, as it's drawn (a few characters are wider that way).
            let cw = ch.width();
            if cur_w > 0 && cur_w.saturating_add(cw) > width {
                out.push(Line::from(std::mem::take(&mut cur)).style(CODE_LINE));
                // The marker only where the character still fits beside it (not at width 2).
                cur_w = 0;
                if cw < width {
                    cur.push(Span::styled(CONTINUATION, marker));
                    cur_w = 1;
                }
            }
            push_merged(&mut cur, ch, span.style);
            cur_w = cur_w.saturating_add(cw);
        }
    }
    out.push(Line::from(cur).style(CODE_LINE));
    out
}

fn push_merged(cur: &mut Vec<Span<'static>>, text: &str, style: Style) {
    match cur.last_mut() {
        Some(last) if last.style == style => last.content.to_mut().push_str(text),
        _ => cur.push(Span::styled(text.to_string(), style)),
    }
}

/// Split into alternating runs of spaces and non-spaces.
fn split_keep_spaces(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(first) = rest.chars().next() {
        let run = rest.find(|c: char| (c == ' ') != (first == ' ')).unwrap_or(rest.len());
        let (word, after) = rest.split_at_checked(run).unwrap_or((rest, ""));
        out.push(word);
        rest = after;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn sample(name: &str) -> String {
        crate::backend::fixture("markup_samples.json")[name].as_str().unwrap().to_string()
    }

    fn code_lines(p: &Parsed) -> Vec<String> {
        p.lines.iter().filter(|l| l.style == CODE_LINE).map(text).collect()
    }

    fn link(board: Option<&str>, thread: Option<u64>, post: Option<u64>) -> Link {
        Link { board: board.map(String::from), thread, post }
    }

    #[test]
    fn hostile_comments_parse_quickly_and_are_cut_off() {
        let distinct: String = (0..150_000).map(|i| format!(">>{i} ")).collect();
        let hostile = [
            "<b>x".repeat(250_000),
            "http://a ".repeat(120_000),
            format!("http://a/{}", ")".repeat(1_000_000)),
            ">>1 ".repeat(250_000),
            distinct,
            "<a href=\"https://x.invalid/\">https://x.invalid/</a> ".repeat(20_000),
        ];
        for html in &hostile {
            let start = std::time::Instant::now();
            let p = parse_html(html, Flavor::Fourchan);
            let took = start.elapsed();
            assert!(took < std::time::Duration::from_secs(1), "{took:?} for {}…", html.get(..20).unwrap_or_default());
            assert!(p.lines.last().is_some_and(|l| text(l).ends_with(CUT_OFF)), "{}…", html.get(..20).unwrap_or_default());
        }
        let p = parse_plain(&"a".repeat(MAX_COMMENT + 1));
        assert!(text(&p.lines[0]).ends_with(CUT_OFF));
        // Cut on a character boundary.
        let p = parse_html(&"é".repeat(MAX_COMMENT), Flavor::Fourchan);
        assert!(text(&p.lines[0]).starts_with("éé"));
        // Past the depth limit tags are ignored; what's under them is still read.
        let deep = format!("{}deep{}", "<b>".repeat(MAX_DEPTH + 10), "</b>".repeat(MAX_DEPTH + 10));
        assert_eq!(text(&parse_html(&deep, Flavor::Fourchan).lines[0]), "deep");
    }

    #[test]
    fn fourchan_html() {
        let p = parse_html(
            r##"<a href="#p123" class="quotelink">&gt;&gt;123</a><br><span class="quote">&gt;be me</span><br>it&#039;s fine<wbr>ok"##,
            Flavor::Fourchan,
        );
        let lines: Vec<_> = p.lines.iter().map(text).collect();
        assert_eq!(lines, [">>123", ">be me", "it's fineok"]);
        assert_eq!(p.quotes, [123]);
        assert_eq!(p.links, [link(None, None, Some(123))]);
        assert_eq!(p.lines[1].spans[0].style, GREENTEXT);
    }

    #[test]
    fn web_links() {
        let spans = |p: &Parsed| -> Vec<(String, bool)> {
            p.lines[0].spans.iter().map(|s| (s.content.to_string(), s.style.fg == Some(mark::LINK))).collect()
        };
        // 4chan doesn't linkify, and splits long words with <wbr>; trailing punctuation and
        // unmatched brackets stay out of the URL.
        let p = parse_html(
            "see https://example.com/a<wbr>bc/def. and (https://en.wikipedia.org/wiki/Rust_(lang)), <s>https://secret.example/x</s>",
            Flavor::Fourchan,
        );
        assert_eq!(p.urls, ["https://example.com/abc/def", "https://en.wikipedia.org/wiki/Rust_(lang)", "https://secret.example/x"]);
        let s = spans(&p);
        assert_eq!(s[0], ("see ".into(), false));
        assert_eq!(s[1], ("https://example.com/abc/def".into(), true));
        assert_eq!(s[2], (". and (".into(), false));
        // A spoilered URL is collected but stays hidden.
        assert!(is_spoiler(p.lines[0].spans.last().unwrap().style));
        // jschan and makaba wrap URLs in links (entities in the href are decoded).
        let p = parse_html(
            r#"<a rel="nofollow" referrerpolicy="same-origin" target="_blank" href="https://z.io/x?a=1&amp;b=2">https://z.io/x?a=1&amp;b=2</a>"#,
            Flavor::Jschan,
        );
        assert_eq!((p.urls.as_slice(), p.links.len()), (&["https://z.io/x?a=1&b=2".to_string()][..], 0));
        assert_eq!(spans(&p), [("https://z.io/x?a=1&b=2".into(), true)]);
        let p = parse_html(r#"<a href="https://youtu.be/abc" target="_blank" rel="nofollow noopener noreferrer">https://youtu.be/abc</a>"#, Flavor::Makaba);
        assert_eq!(p.urls, ["https://youtu.be/abc"]);
        // Quote links, even with absolute hrefs, aren't web links.
        for (html, flavor) in [
            (r#"<a href="//boards.4chan.org/g/thread/1#p2" class="quotelink">&gt;&gt;&gt;/g/2</a>"#, Flavor::Fourchan),
            (r#"<a class="quoteLink" href="https://endchan.net/b/res/1.html#2">&gt;&gt;2</a>"#, Flavor::Lynxchan),
            (r#"<a class="quote" href="https://zzzchan.xyz/b/thread/1.html#2">&gt;&gt;2</a>"#, Flavor::Jschan),
            (r#"<a href="https://2ch.hk/b/res/1.html#2" class="post-reply-link" data-thread="1" data-num="2">&gt;&gt;2</a>"#, Flavor::Makaba),
        ] {
            let p = parse_html(html, flavor);
            assert!(p.urls.is_empty() && p.links.len() == 1, "{html}");
        }
        // No URLs from code blocks or bare schemes.
        assert!(parse_html("<pre>curl https://x.example/</pre> https:// http:///", Flavor::Fourchan).urls.is_empty());
    }

    #[test]
    fn plain() {
        let p = parse_plain(">>42 hello\n>green\n<pink\n>>>/b/7 and >>>/tech/");
        assert_eq!(p.quotes, [42]);
        assert_eq!(p.lines[1].spans[0].style, GREENTEXT);
        assert_eq!(p.lines[2].spans[0].style, PINKTEXT);
        assert_eq!(p.lines[0].spans[0].style, QUOTELINK);
        assert_eq!(p.links[1..], [link(Some("b"), None, Some(7)), link(Some("tech"), None, None)]);
    }

    #[test]
    fn adjacent_quotes_stay_separate_spans() {
        let p = parse_plain(">>1>>2 x");
        let spans: Vec<_> = p.lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(spans, [">>1", ">>2", " x"]);
        assert_eq!(quote_target(">>12"), Some(12));
        assert_eq!(quote_target(">>12 (OP)"), None);
    }

    #[test]
    fn hrefs() {
        assert_eq!(parse_href("#p5"), link(None, None, Some(5)));
        assert_eq!(parse_href("/g/thread/109953009#p109953010"), link(Some("g"), Some(109953009), Some(109953010)));
        assert_eq!(parse_href("//boards.4chan.org/biz/thread/1#p2"), link(Some("biz"), Some(1), Some(2)));
        assert_eq!(parse_href("//boards.4chan.org/wsr/"), link(Some("wsr"), None, None));
        assert_eq!(parse_href("//boards.4chan.org/g/catalog#s=lmg%2F"), link(Some("g"), None, None));
        assert_eq!(parse_href("/λ/res/31826.html#44350"), link(Some("λ"), Some(31826), Some(44350)));
        assert_eq!(parse_href("/%CE%BB/res/1.html#q2"), link(Some("λ"), Some(1), Some(2)));
        assert_eq!(parse_href("https://desuarchive.org/g/thread/9/#10"), link(Some("g"), Some(9), Some(10)));
        assert_eq!(parse_href("/g/post/10/"), link(Some("g"), None, Some(10)));
    }

    #[test]
    fn fourchan_samples() {
        // Cross-thread: the href carries the other thread's number.
        let p = parse_html(&sample("4chan_cross_thread"), Flavor::Fourchan);
        assert!(p.links.contains(&link(Some("g"), Some(109953009), Some(109953009))));

        // Spoilers are <s> on 4chan.
        let p = parse_html(&sample("4chan_spoiler"), Flavor::Fourchan);
        assert!(p.lines[1].spans.iter().all(|s| is_spoiler(s.style)));

        // Code keeps its indentation and is marked as code; the text around it isn't.
        let p = parse_html(&sample("4chan_code"), Flavor::Fourchan);
        let code = code_lines(&p);
        assert!(code.contains(&"  if(player.disconnected)".to_string()), "{code:?}");
        assert!(code.contains(&"void UpdatePlayer(Player& player)".to_string()));
        assert_eq!(text(&p.lines[1]), ">muh readability");
        assert_ne!(p.lines[1].style, CODE_LINE);
    }

    #[test]
    fn vichan_samples() {
        let p = parse_html(&sample("vichan_cross_board"), Flavor::Vichan);
        // `>>>/lambda/44350` uses an alias; the href's canonical board wins.
        assert!(p.links.contains(&link(Some("λ"), Some(31826), Some(44350))));
        assert!(!p.links.iter().any(|l| l.board.as_deref() == Some("lambda")));

        let p = parse_html(&sample("vichan_spoiler"), Flavor::Vichan);
        let spoiler: Vec<_> = p.lines.iter().flat_map(|l| &l.spans).filter(|s| is_spoiler(s.style)).collect();
        assert_eq!(spoiler[0].content, "SaaS");

        let p = parse_html(&sample("vichan_code"), Flavor::Vichan);
        let code = code_lines(&p);
        assert!(code.iter().any(|l| l.starts_with("(define (fizz-buzz i)")), "{code:?}");
        assert!(code.iter().any(|l| l.starts_with("  (cond ((divides? i 3) \"Fizz\")")), "{code:?}");
    }

    #[test]
    fn lynxchan_samples() {
        // kohlchan: raw newlines, hljs code blocks.
        let p = parse_html(&sample("lynx_kohl_code"), Flavor::Lynxchan);
        assert_eq!(code_lines(&p)[0], "dosbox-x.exe --set machine=pc98");
        assert!(p.lines.len() > 3);

        let p = parse_html(&sample("lynx_kohl_spoiler"), Flavor::Lynxchan);
        let spoiler = p.lines.iter().flat_map(|l| &l.spans).find(|s| is_spoiler(s.style)).unwrap();
        assert!(spoiler.content.starts_with("Achtung"));
        assert!(p.lines.iter().flat_map(|l| &l.spans).any(|s| s.style == HEADING));

        // endchan: <br> line breaks, <pre> used inline inside greentext.
        let p = parse_html(&sample("lynx_end_pre"), Flavor::Lynxchan);
        let code = code_lines(&p);
        assert!(code.contains(&"about:debugging#/runtime/this-firefox".to_string()), "{code:?}");
    }

    #[test]
    fn wrapping() {
        let l = Line::from("aaa bbb ccc");
        let w: Vec<_> = wrap(&l, 7).iter().map(text).collect();
        assert_eq!(w, ["aaa bbb", "ccc"]);
        let w: Vec<_> = wrap(&Line::from("abcdefghij"), 4).iter().map(text).collect();
        assert_eq!(w, ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn text_every_terminal_measures_alike() {
        use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
        for (raw, safe) in [
            ("family 👨\u{200d}👩\u{200d}👧 here", "family 👨 here"),
            ("ok 👍🏽", "ok 👍"),
            ("love ❤\u{fe0f}", "love ❤"),
            ("🇯🇵 and 🇺🇸", "JP and US"),
            ("1\u{fe0f}\u{20e3}", "1"),
            ("plain text, 日本語, é", "plain text, 日本語, é"),
            ("cat\u{202e}gpj.exe", "catgpj.exe"),
            ("\u{2067}isolated\u{2069} and \u{202a}embedded\u{202c}", "isolated and embedded"),
        ] {
            let got = for_terminal(raw);
            assert_eq!(got, safe);
            // Measured whole or a character at a time, the same: never wider than before.
            let by_char: usize = got.chars().map(|c| c.width().unwrap_or(0)).sum();
            assert_eq!(got.width(), by_char, "{got:?}");
            assert!(got.width() <= raw.width(), "{raw:?}");
        }
    }

    #[test]
    fn links_know_where_they_are() {
        let html = r##"<a href="#p123" class="quotelink">&gt;&gt;123</a> see <a href="https://x.org/a">this page</a> and https://y.org/b<br>&gt;&gt;&gt;/g/456 <a href="https://y.org/b">https://y.org/b</a>"##;
        let p = parse_html(html, Flavor::Fourchan);
        let at = |a: &Anchor| {
            let line: String = p.lines[a.line].spans.iter().map(|s| s.content.as_ref()).collect();
            (a.line, line.get(a.start..a.end).unwrap_or_default().to_string())
        };
        let found: Vec<_> = p.anchors.iter().map(|a| (at(a), a.to.clone())).collect();
        let quote = |board: Option<&str>, post| Target::Quote(Link { board: board.map(String::from), thread: None, post: Some(post) });
        let url = |u: &str| Target::Url(u.into());
        assert_eq!(
            found,
            [
                ((0, ">>123".into()), quote(None, 123)),
                ((0, "this page".into()), url("https://x.org/a")),
                ((0, "https://y.org/b".into()), url("https://y.org/b")),
                ((1, ">>>/g/456".into()), quote(Some("g"), 456)),
                ((1, "https://y.org/b".into()), url("https://y.org/b")),
            ]
        );
    }

    #[test]
    fn code_lines_wrap_with_marker() {
        let l = Line::from("    let x = 1;").style(CODE_LINE);
        let w: Vec<_> = wrap(&l, 8).iter().map(text).collect();
        assert_eq!(w, ["    let ", "↪x = 1;"]);
        assert!(wrap(&l, 8).iter().all(|l| l.style == CODE_LINE));
        // Too narrow for the marker and a wide character: no marker, nothing wider than 2.
        let w: Vec<_> = wrap(&Line::from("😀😀").style(CODE_LINE), 2).iter().map(text).collect();
        assert_eq!(w, ["😀", "😀"]);
    }

    #[test]
    fn highlighting() {
        let l = Line::from(vec![Span::raw("Foo bar foo"), Span::styled("foo", SPOILER)]);
        let hl = Style::new().bg(Color::Yellow);
        let h = highlight(&l, "foo", hl);
        let marked: Vec<_> = h.spans.iter().filter(|s| s.style == hl).map(|s| s.content.as_ref()).collect();
        assert_eq!(marked, ["Foo", "foo"]);
    }
}
