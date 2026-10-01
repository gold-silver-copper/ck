//! Turning post comments into styled ratatui lines, and wrapping them.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

pub const GREENTEXT: Style = Style::new().fg(Color::Green);
pub const PINKTEXT: Style = Style::new().fg(Color::LightRed);
pub const QUOTELINK: Style = Style::new().fg(Color::Magenta).add_modifier(Modifier::UNDERLINED);
pub const SPOILER: Style = Style::new().fg(Color::DarkGray).bg(Color::DarkGray);
pub const HEADING: Style = Style::new().fg(Color::Red).add_modifier(Modifier::BOLD);
pub const CODE: Style = Style::new().fg(Color::Cyan);

/// Parsed comment: styled lines plus the post numbers it quotes.
pub struct Parsed {
    pub lines: Vec<Line<'static>>,
    pub quotes: Vec<u64>,
}

/// Parse the HTML subset used by 4chan and vichan comments.
pub fn parse_html(html: &str) -> Parsed {
    let mut b = Builder::default();
    let mut stack: Vec<(String, Style)> = Vec::new();
    let mut rest = html;

    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            b.text(&decode(rest), current(&stack));
            break;
        };
        if lt > 0 {
            b.text(&decode(&rest[..lt]), current(&stack));
        }
        let Some(gt) = rest[lt..].find('>') else {
            // Unterminated tag: treat the remainder as text.
            b.text(&decode(&rest[lt..]), current(&stack));
            break;
        };
        let tag = &rest[lt + 1..lt + gt];
        rest = &rest[lt + gt + 1..];

        let closing = tag.starts_with('/');
        let tag_body = tag.trim_start_matches('/').trim_end_matches('/');
        let name = tag_body
            .split(|c: char| c.is_whitespace())
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();

        match name.as_str() {
            "br" => b.newline(),
            "wbr" => {}
            "p" | "div" | "pre" if closing => {
                pop(&mut stack, &name);
                b.newline();
            }
            _ if closing => pop(&mut stack, &name),
            _ => {
                let base = current(&stack);
                let class = attr(tag_body, "class").unwrap_or_default();
                let style = match name.as_str() {
                    "a" => base.patch(QUOTELINK),
                    "b" | "strong" => base.add_modifier(Modifier::BOLD),
                    "i" | "em" => base.add_modifier(Modifier::ITALIC),
                    "u" => base.add_modifier(Modifier::UNDERLINED),
                    "s" | "del" | "strike" => base.add_modifier(Modifier::CROSSED_OUT),
                    "code" | "pre" => base.patch(CODE),
                    "span" | "div" | "p" => match class.as_str() {
                        c if c.contains("quote") => base.patch(GREENTEXT),
                        c if c.contains("spoiler") => base.patch(SPOILER),
                        c if c.contains("heading") => base.patch(HEADING),
                        _ => base,
                    },
                    _ => base,
                };
                stack.push((name, style));
            }
        }
    }
    b.finish()
}

/// Parse LynxChan's plain-text `message` field (markup is in-band).
pub fn parse_plain(text: &str) -> Parsed {
    let mut b = Builder::default();
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            b.newline();
        }
        let is_quote_link = line.starts_with(">>") && line[2..].starts_with(|c: char| c.is_ascii_digit());
        let style = if line.starts_with('>') && !is_quote_link {
            GREENTEXT
        } else if line.starts_with('<') {
            PINKTEXT
        } else if line.starts_with("==") && line.ends_with("==") && line.len() > 4 {
            HEADING
        } else {
            Style::new()
        };
        b.text(line, style);
    }
    b.finish()
}

#[derive(Default)]
struct Builder {
    lines: Vec<Line<'static>>,
    cur: Vec<Span<'static>>,
    quotes: Vec<u64>,
}

impl Builder {
    fn text(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        // Highlight `>>123` quote links appearing in text, and record them.
        let mut rest = text;
        while let Some(pos) = rest.find(">>") {
            let digits: String = rest[pos + 2..].chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                self.push(&rest[..pos + 2], style);
                rest = &rest[pos + 2..];
                continue;
            }
            self.push(&rest[..pos], style);
            let end = pos + 2 + digits.len();
            self.push(&rest[pos..end], style.patch(QUOTELINK));
            if let Ok(n) = digits.parse() {
                if !self.quotes.contains(&n) {
                    self.quotes.push(n);
                }
            }
            rest = &rest[end..];
        }
        self.push(rest, style);
    }

    fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        match self.cur.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push_str(text),
            _ => self.cur.push(Span::styled(text.to_string(), style)),
        }
    }

    fn newline(&mut self) {
        self.lines.push(Line::from(std::mem::take(&mut self.cur)));
    }

    fn finish(mut self) -> Parsed {
        if !self.cur.is_empty() {
            self.newline();
        }
        // Drop trailing blank lines.
        while self.lines.last().is_some_and(|l| l.width() == 0) {
            self.lines.pop();
        }
        Parsed { lines: self.lines, quotes: self.quotes }
    }
}

fn current(stack: &[(String, Style)]) -> Style {
    stack.last().map(|(_, s)| *s).unwrap_or_default()
}

fn pop(stack: &mut Vec<(String, Style)>, name: &str) {
    if let Some(i) = stack.iter().rposition(|(n, _)| n == name) {
        stack.truncate(i);
    }
}

fn attr(tag: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let value = match rest.chars().next()? {
        q @ ('"' | '\'') => rest[1..].split(q).next()?,
        _ => rest.split(|c: char| c.is_whitespace()).next()?,
    };
    Some(value.to_string())
}

/// Decode HTML entities (also used for subjects and names).
pub fn decode(s: &str) -> String {
    html_escape::decode_html_entities(s).into_owned()
}

/// Word-wrap a styled line to `width` columns. Over-long words are split.
pub fn wrap(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0;

    let push = |cur: &mut Vec<Span<'static>>, text: &str, style: Style| match cur.last_mut() {
        Some(last) if last.style == style => last.content.to_mut().push_str(text),
        _ => cur.push(Span::styled(text.to_string(), style)),
    };

    for span in &line.spans {
        for token in split_keep_spaces(&span.content) {
            let tw = token.width();
            let is_space = token.starts_with(' ');
            if cur_w + tw <= width {
                push(&mut cur, token, span.style);
                cur_w += tw;
            } else if is_space {
                // Break at whitespace; don't carry it to the next line.
                out.push(Line::from(std::mem::take(&mut cur)));
                cur_w = 0;
            } else if tw <= width {
                out.push(Line::from(std::mem::take(&mut cur)));
                push(&mut cur, token, span.style);
                cur_w = tw;
            } else {
                for ch in token.chars() {
                    let cw = ch.to_string().width();
                    if cur_w + cw > width {
                        out.push(Line::from(std::mem::take(&mut cur)));
                        cur_w = 0;
                    }
                    push(&mut cur, &ch.to_string(), span.style);
                    cur_w += cw;
                }
            }
        }
    }
    out.push(Line::from(cur));
    out
}

/// Split into alternating runs of spaces and non-spaces.
fn split_keep_spaces(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_space = None;
    for (i, c) in s.char_indices() {
        let sp = c == ' ';
        if in_space.is_some_and(|prev| prev != sp) {
            out.push(&s[start..i]);
            start = i;
        }
        in_space = Some(sp);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn fourchan_html() {
        let p = parse_html(
            r##"<a href="#p123" class="quotelink">&gt;&gt;123</a><br><span class="quote">&gt;be me</span><br>it&#039;s fine<wbr>ok"##,
        );
        let lines: Vec<_> = p.lines.iter().map(text).collect();
        assert_eq!(lines, [">>123", ">be me", "it's fineok"]);
        assert_eq!(p.quotes, [123]);
        assert_eq!(p.lines[1].spans[0].style, GREENTEXT);
    }

    #[test]
    fn plain() {
        let p = parse_plain(">>42 hello\n>green\n<pink");
        assert_eq!(p.quotes, [42]);
        assert_eq!(p.lines[1].spans[0].style, GREENTEXT);
        assert_eq!(p.lines[2].spans[0].style, PINKTEXT);
        assert_eq!(p.lines[0].spans[0].style, QUOTELINK);
    }

    #[test]
    fn wrapping() {
        let l = Line::from("aaa bbb ccc");
        let w: Vec<_> = wrap(&l, 7).iter().map(text).collect();
        assert_eq!(w, ["aaa bbb", "ccc"]);
        let w: Vec<_> = wrap(&Line::from("abcdefghij"), 4).iter().map(text).collect();
        assert_eq!(w, ["abcd", "efgh", "ij"]);
    }
}
