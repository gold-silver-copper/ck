//! `E`: a thread saved as a page (thread.html, readable offline) and as data (thread.json).

use std::path::Path;

use anyhow::{Context, Result};
use html_escape::{encode_double_quoted_attribute as attr, encode_text as text};
use ratatui::style::{Color, Modifier, Style};
use serde_json::json;

use crate::download;
use crate::markup;
use crate::model::Post;
use crate::theme::{Theme, mark};

/// Where and what a thread is, for the saved copies.
pub struct About<'a> {
    pub site: &'a str,
    pub board: &'a str,
    pub thread: u64,
    pub url: &'a str,
    pub saved: i64,
}

/// Write thread.html and thread.json into `dir` (replacing earlier copies).
pub fn save(posts: &[Post], about: &About, theme: &Theme, dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    write(&dir.join("thread.html"), html(posts, about, theme, dir).as_bytes())?;
    write(&dir.join("thread.json"), &serde_json::to_vec_pretty(&data(posts, about, dir))?)
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
}

/// Each file of a post with the name it has (or would have) after `d`/`D`, and whether
/// it's been downloaded into `dir`.
fn files<'a>(p: &'a Post, dir: &Path) -> Vec<(&'a crate::model::Attachment, String, bool)> {
    let jobs = download::jobs(&[p], dir);
    p.files
        .iter()
        .zip(jobs)
        .map(|(f, (_, path))| {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            (f, name, path.exists())
        })
        .collect()
}

/// ck's own model of the thread. `format` changes if the shape does.
pub fn data(posts: &[Post], about: &About, dir: &Path) -> serde_json::Value {
    let posts: Vec<_> = posts
        .iter()
        .map(|p| {
            let files: Vec<_> = files(p, dir)
                .into_iter()
                .map(|(f, name, saved)| {
                    json!({
                        "filename": f.filename, "url": f.url, "thumb": f.thumb, "spoiler": f.spoiler,
                        "width": f.width, "height": f.height, "size": f.size, "md5": f.md5,
                        "saved_as": saved.then_some(name),
                    })
                })
                .collect();
            json!({
                "no": p.no, "name": p.name, "subject": p.subject, "time": p.time,
                "text": crate::app::copy_text(p, false), "quotes": p.quotes, "urls": p.urls, "files": files,
            })
        })
        .collect();
    json!({
        "format": 1, "site": about.site, "board": about.board, "thread": about.thread,
        "url": about.url, "saved": about.saved, "posts": posts,
    })
}

/// A CSS color for a theme color. Terminal palette colors get their usual RGB.
fn css(c: Color, fallback: &str) -> String {
    let rgb = |r: u8, g: u8, b: u8| format!("#{r:02x}{g:02x}{b:02x}");
    match c {
        Color::Rgb(r, g, b) => rgb(r, g, b),
        Color::Black => rgb(0, 0, 0),
        Color::Red => rgb(205, 49, 49),
        Color::Green => rgb(13, 188, 121),
        Color::Yellow => rgb(229, 229, 16),
        Color::Blue => rgb(36, 114, 200),
        Color::Magenta => rgb(188, 63, 188),
        Color::Cyan => rgb(17, 168, 205),
        Color::Gray => rgb(204, 204, 204),
        Color::DarkGray => rgb(118, 118, 118),
        Color::LightRed => rgb(241, 76, 76),
        Color::LightGreen => rgb(35, 209, 139),
        Color::LightYellow => rgb(245, 245, 67),
        Color::LightBlue => rgb(59, 142, 234),
        Color::LightMagenta => rgb(214, 112, 214),
        Color::LightCyan => rgb(41, 184, 219),
        Color::White => rgb(255, 255, 255),
        _ => fallback.to_string(),
    }
}

/// The thread as one HTML page with its styles inline: the current theme's colors, posts
/// as cards, quotes linking within the page, files from the folder when downloaded.
pub fn html(posts: &[Post], about: &About, t: &Theme, dir: &Path) -> String {
    let title = posts.first().map(|op| crate::app::thread_subject(std::slice::from_ref(op))).unwrap_or_default();
    let numbers: std::collections::HashSet<u64> = posts.iter().map(|p| p.no).collect();
    let c = |c: Color, f: &str| css(c, f);
    let mut out = format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title_t}</title>
<style>
body {{ margin: 0; padding: 24px 16px; background: {bg}; color: {fg}; font: 15px/1.5 system-ui, sans-serif; }}
main {{ max-width: 860px; margin: 0 auto; }}
header {{ background: {bar}; color: {on_bar}; padding: 12px 16px; margin-bottom: 16px; }}
header a {{ color: {primary}; }}
.post {{ background: {surface}; padding: 12px 16px; margin-bottom: 12px; overflow: hidden; }}
.post:target {{ background: {selection}; box-shadow: inset 4px 0 {primary}; }}
.head {{ color: {dim}; font-size: 13px; }}
.name {{ color: {name}; font-weight: bold; }}
.subject {{ color: {primary}; font-weight: bold; }}
.op {{ background: {pc}; color: {on_pc}; padding: 0 6px; }}
.files {{ float: left; margin: 4px 16px 4px 0; }}
.files img {{ max-width: 200px; max-height: 200px; display: block; }}
.file {{ font-size: 13px; color: {dim}; }}
.body {{ white-space: pre-wrap; word-wrap: break-word; }}
.greentext {{ color: {green}; }} .pinktext {{ color: {pink}; }} .heading {{ color: {heading}; font-weight: bold; }}
.quote {{ color: {quote}; }} a {{ color: {quote}; }}
pre {{ background: {code_bg}; color: {code}; padding: 8px; overflow-x: auto; white-space: pre; margin: 4px 0; }}
.spoiler {{ background: {spoiler}; color: {spoiler}; }} .spoiler:hover {{ color: {fg}; }}
.replies {{ color: {dim}; font-size: 13px; }}
</style></head><body><main>
<header><b>{title_t}</b><br>{site} · /{board}/ · <a href="{url}">{url_t}</a></header>
"#,
        title_t = text(&title),
        site = text(about.site),
        board = text(about.board),
        url = attr(about.url),
        url_t = text(about.url),
        bg = c(t.background, "#121212"),
        fg = c(t.text, "#e0e0e0"),
        bar = c(t.bar, "#202020"),
        on_bar = c(t.on_bar, "#e0e0e0"),
        primary = c(t.primary, "#a0a0ff"),
        surface = c(t.surface, "#1c1c1c"),
        selection = c(t.selection, "#303040"),
        dim = c(t.text_dim, "#909090"),
        name = c(t.name, "#80c080"),
        pc = c(t.primary_container, "#404070"),
        on_pc = c(t.on_primary_container, "#ffffff"),
        green = c(t.greentext, "#90c070"),
        pink = c(t.pinktext, "#e0a080"),
        heading = c(t.heading, "#e08080"),
        quote = c(t.quotelink, "#e0a0c0"),
        code_bg = c(t.code_bg, "#0a0a0a"),
        code = c(t.code, "#a0c0ff"),
        spoiler = c(t.spoiler, "#404040"),
    );
    let mut backlinks: std::collections::HashMap<u64, Vec<u64>> = Default::default();
    for p in posts {
        for q in &p.quotes {
            backlinks.entry(*q).or_default().push(p.no);
        }
    }
    for (i, p) in posts.iter().enumerate() {
        let time = chrono::DateTime::from_timestamp(p.time, 0).map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_default();
        out.push_str(&format!(r#"<article class="post" id="p{}"><div class="head"><span class="name">{}</span> "#, p.no, text(&p.name)));
        if i == 0 {
            out.push_str(r#"<span class="op">OP</span> "#);
        }
        out.push_str(&format!(r##"{time} <a href="#p{0}">No.{0}</a></div>"##, p.no));
        if let Some(s) = &p.subject {
            out.push_str(&format!(r#"<div class="subject">{}</div>"#, text(s)));
        }
        let files = files(p, dir);
        if !files.is_empty() {
            out.push_str(r#"<div class="files">"#);
            for (f, name, saved) in &files {
                let href = if *saved { name.clone() } else { f.url.clone() };
                let img = if *saved && f.is_image() { Some(name.clone()) } else { f.thumb.clone() };
                out.push_str(&format!(r#"<a href="{}">"#, attr(&href)));
                if let Some(src) = img {
                    out.push_str(&format!(r#"<img src="{}" alt="{}" loading="lazy">"#, attr(&src), attr(&f.filename)));
                }
                out.push_str(&format!(r#"</a><div class="file">{}</div>"#, text(&f.filename)));
            }
            out.push_str("</div>");
        }
        out.push_str(r#"<div class="body">"#);
        out.push_str(&body_html(p, &numbers));
        out.push_str("</div>");
        if let Some(r) = backlinks.get(&p.no) {
            let links: Vec<String> = r.iter().map(|n| format!(r##"<a href="#p{n}">&gt;&gt;{n}</a>"##)).collect();
            out.push_str(&format!(r#"<div class="replies">Replies: {}</div>"#, links.join(" ")));
        }
        out.push_str("</article>\n");
    }
    out.push_str("</main></body></html>\n");
    out
}

/// A post's comment as HTML, from its parsed lines: classes for the markup, anchors for
/// quotes of posts on the page, links for web links, `<pre>` for code.
fn body_html(p: &Post, numbers: &std::collections::HashSet<u64>) -> String {
    let mut out = String::new();
    let mut in_code = false;
    for (k, line) in p.body.iter().enumerate() {
        let code = line.style == markup::CODE_LINE;
        if code && !in_code {
            out.push_str("<pre>");
        } else if !code && in_code {
            out.push_str("</pre>");
        } else if k > 0 {
            out.push('\n');
        }
        in_code = code;
        for s in &line.spans {
            out.push_str(&span_html(&s.content, s.style, numbers));
        }
    }
    if in_code {
        out.push_str("</pre>");
    }
    out
}

fn span_html(content: &str, style: Style, numbers: &std::collections::HashSet<u64>) -> String {
    let body = text(content).into_owned();
    let mut html = if markup::is_quote_link(style) {
        match markup::quote_target(content).filter(|n| numbers.contains(n)) {
            Some(n) => format!(r##"<a class="quote" href="#p{n}">{body}</a>"##),
            None => format!(r#"<span class="quote">{body}</span>"#),
        }
    } else if style.fg == Some(mark::LINK) {
        format!(r#"<a href="{}">{body}</a>"#, attr(content))
    } else if markup::is_spoiler(style) {
        format!(r#"<span class="spoiler">{body}</span>"#)
    } else {
        let class = match style.fg {
            Some(mark::GREENTEXT) => Some("greentext"),
            Some(mark::PINKTEXT) => Some("pinktext"),
            Some(mark::HEADING) => Some("heading"),
            _ => None,
        };
        match class {
            Some(c) => format!(r#"<span class="{c}">{body}</span>"#),
            None => body,
        }
    };
    for (m, tag) in [(Modifier::BOLD, "b"), (Modifier::ITALIC, "i"), (Modifier::CROSSED_OUT, "s")] {
        if style.add_modifier.contains(m) && !markup::is_quote_link(style) {
            html = format!("<{tag}>{html}</{tag}>");
        }
    }
    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::{Flavor, parse_html};
    use crate::model::Attachment;

    fn posts() -> Vec<Post> {
        let post = |no, html: &str| {
            let p = parse_html(html, Flavor::Fourchan);
            Post { no, name: "Anonymous".into(), time: 1_790_000_000, body: p.lines, quotes: p.quotes, links: p.links, urls: p.urls, ..Default::default() }
        };
        let mut op = post(1, "Hello <b>world</b> &amp; <s>secret</s><br><span class=\"quote\">&gt;green</span>");
        op.subject = Some("A <thread>".into());
        op.files = vec![Attachment { filename: "cat.png".into(), url: "https://i.example/1.png".into(), thumb: Some("https://i.example/1s.jpg".into()), ..Default::default() }];
        vec![op, post(2, "<a href=\"#p1\" class=\"quotelink\">&gt;&gt;1</a> see https://example.com/x<br><pre>fn main() {<br>}</pre>")]
    }

    #[test]
    fn saves_html_and_json() {
        let dir = tempfile::tempdir().unwrap();
        let about = About { site: "4chan", board: "g", thread: 1, url: "https://boards.4chan.org/g/thread/1", saved: 0 };
        save(&posts(), &about, &crate::theme::theme(), dir.path()).unwrap();
        let html = std::fs::read_to_string(dir.path().join("thread.html")).unwrap();
        // Escaped text, markup classes, quotes linking within the page, web links, code.
        assert!(html.contains("A &lt;thread&gt;"), "{html}");
        assert!(html.contains("<b>world</b> &amp; <span class=\"spoiler\">secret</span>"), "{html}");
        assert!(html.contains("<span class=\"greentext\">&gt;green</span>"));
        assert!(html.contains(r##"<a class="quote" href="#p1">&gt;&gt;1</a>"##));
        assert!(html.contains(r#"<a href="https://example.com/x">https://example.com/x</a>"#));
        assert!(html.contains("<pre>fn main() {\n}</pre>"), "{html}");
        assert!(html.contains(r##"Replies: <a href="#p2">"##));
        // Not downloaded: the file links to the site, with its thumbnail.
        assert!(html.contains(r#"<a href="https://i.example/1.png"><img src="https://i.example/1s.jpg""#));
        // Downloaded (d/D first): the local file.
        std::fs::write(dir.path().join("1_cat.png"), b"x").unwrap();
        save(&posts(), &about, &crate::theme::theme(), dir.path()).unwrap();
        let html = std::fs::read_to_string(dir.path().join("thread.html")).unwrap();
        assert!(html.contains(r#"<a href="1_cat.png"><img src="1_cat.png""#));
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.path().join("thread.json")).unwrap()).unwrap();
        assert_eq!(v["format"], 1);
        assert_eq!(v["posts"][1]["quotes"], json!([1]));
        assert_eq!(v["posts"][0]["files"][0]["saved_as"], "1_cat.png");
        assert_eq!(v["posts"][0]["text"], "Hello world & secret\n>green");
        assert!(!dir.path().join("thread.tmp").exists());
    }
}
