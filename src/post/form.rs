//! A page's post form, read as a browser would send it: what's needed where a site checks
//! that its form came back whole (vichan's hidden anti-spam fields).

use std::sync::LazyLock;

use regex::Regex;

/// A form as it would be sent: its fields (hidden ones too, and the text boxes as they are),
/// its submit button's value, and the name of its file input.
#[derive(Debug, Default, PartialEq)]
pub struct Form {
    pub fields: Vec<(String, String)>,
    pub submit: Option<String>,
    pub file: Option<String>,
}

impl Form {
    /// The form named `name` in `html`.
    pub fn find(html: &str, name: &str) -> Option<Form> {
        static FORM: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"(?is)<form\b([^>]*)>(.*?)</form>").ok());
        static TAG: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"(?is)<(input|textarea|select|option)\b([^>]*)>").ok());
        let (form_re, tag_re) = (FORM.as_ref()?, TAG.as_ref()?);
        let inner = form_re.captures_iter(html).find(|c| c.get(1).is_some_and(|a| attr(a.as_str(), "name").as_deref() == Some(name)))?.get(2)?;
        let html = inner.as_str();
        let mut form = Form::default();
        // The select being read, and how far its value is settled: by its first option, then
        // by a selected one.
        let mut select: Option<(String, u8)> = None;
        for tag in tag_re.captures_iter(html) {
            let (Some(kind), Some(attrs), Some(whole)) = (tag.get(1), tag.get(2), tag.get(0)) else { continue };
            let attrs = attrs.as_str();
            let value = || attr(attrs, "value").unwrap_or_default();
            match kind.as_str().to_ascii_lowercase().as_str() {
                "option" => {
                    let Some((name, settled)) = &mut select else { continue };
                    let by = if has(attrs, "selected") { 2 } else { 1 };
                    if by > *settled {
                        form.fields.retain(|(k, _)| k != name);
                        form.fields.push((name.clone(), value()));
                        *settled = by;
                    }
                    continue;
                }
                "select" => {
                    select = attr(attrs, "name").map(|n| (n, 0));
                    continue;
                }
                _ => {}
            }
            let Some(name) = attr(attrs, "name") else { continue };
            if kind.as_str().eq_ignore_ascii_case("textarea") {
                let rest = html.get(whole.end()..).unwrap_or_default();
                let text = rest.find("</textarea>").and_then(|end| rest.get(..end)).unwrap_or_default();
                form.fields.push((name, crate::markup::decode(text)));
                continue;
            }
            match attr(attrs, "type").unwrap_or_default().to_ascii_lowercase().as_str() {
                "file" => {
                    form.file.get_or_insert(name);
                }
                "submit" => {
                    form.submit.get_or_insert_with(value);
                }
                "checkbox" | "radio" if !has(attrs, "checked") => {}
                "button" | "image" | "reset" => {}
                _ => form.fields.push((name, value())),
            }
        }
        Some(form)
    }

    /// Whether it has a field `name`.
    pub fn has(&self, name: &str) -> bool {
        self.fields.iter().any(|(k, _)| k == name)
    }

    /// Set `name` to `value`, where it was or at the end.
    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        match self.fields.iter_mut().find(|(k, _)| k == name) {
            Some(field) => field.1 = value,
            None => self.fields.push((name.into(), value)),
        }
    }
}

/// Attribute `name` of a tag's attributes, its entities decoded.
fn attr(attrs: &str, name: &str) -> Option<String> {
    static ATTR: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r#"([^\s=/>"']+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+)))?"#).ok());
    ATTR.as_ref()?.captures_iter(attrs).find(|c| c.get(1).is_some_and(|k| k.as_str().eq_ignore_ascii_case(name))).map(|c| {
        let v = c.get(2).or_else(|| c.get(3)).or_else(|| c.get(4)).map_or("", |m| m.as_str());
        crate::markup::decode(v)
    })
}

/// Whether a tag has attribute `name` (as `checked`, with or without a value).
fn has(attrs: &str, name: &str) -> bool {
    attr(attrs, name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vichan_form_comes_back_whole() {
        let html = std::fs::read_to_string("tests/fixtures/vichan_wizchan_form.html").unwrap();
        let form = Form::find(&html, "post").unwrap();
        let get = |k: &str| form.fields.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        // Hidden decoys, entities decoded; text boxes as they are; the select's first option.
        assert_eq!(get("lastname"), Some("⛁epu,⛠XfjUlI-(HzBK"));
        assert_eq!(get("q"), Some(")PWrRs+wl☗>{vFC=?aB.-E[ D"));
        assert_eq!(get("mt31lbfv☑9xejpzswk0rq4n2⛕7⚜ohy"), Some(""));
        assert_eq!(get("email"), Some(""));
        assert_eq!(get("body"), Some(""));
        assert_eq!(get("hash"), Some("e7759aaa77dd0d0dee866362f39cfb734acaedfe"));
        // Not sent: the unticked spoiler, the file input, the submit button.
        assert_eq!(get("spoiler"), None);
        assert_eq!((form.submit.as_deref(), form.file.as_deref()), (Some("New Wisdom"), Some("file[]")));
        assert!(form.fields.iter().all(|(n, _)| n != "post" && n != "file[]"));
    }
}
