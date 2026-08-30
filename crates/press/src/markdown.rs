//! Page source to HTML: pulldown-cmark with the site's conventions applied
//! in the event stream - fenced blocks dispatched to renderers, heading ids,
//! GFM autolinks and footnotes in the shape the site's CSS knows.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;

use pulldown_cmark::{html, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::blocks::{self, Attrs, Env};
use crate::highlight::Highlighter;

pub struct Markdown<'a> {
    pub root: &'a Path,
    pub cache_dir: &'a Path,
    pub smart_punctuation: bool,
    pub highlighter: &'a Highlighter,
}

/// A rendered body plus what it turned out to need: the client-side
/// libraries are derived from the blocks actually present, not declared by
/// hand in front matter.
pub struct Rendered {
    pub html: String,
    pub scripts: Vec<String>,
}

impl<'a> Markdown<'a> {
    pub fn render(&self, src: &str) -> Result<Rendered, String> {
        let scripts = RefCell::new(Vec::new());
        let html = self.render_into(src, &scripts)?;
        let mut scripts = scripts.into_inner();
        scripts.sort();
        scripts.dedup();
        Ok(Rendered { html, scripts })
    }

    /// Blocks whose body is Markdown (`note`) come back through here.
    fn render_into(&self, src: &str, scripts: &RefCell<Vec<String>>) -> Result<String, String> {
        let mut options = Options::ENABLE_TABLES | Options::ENABLE_FOOTNOTES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
        if self.smart_punctuation {
            options |= Options::ENABLE_SMART_PUNCTUATION;
        }
        let highlight = |code: &str, lang: &str| self.highlighter.html(code, lang);
        let fragment = |body: &str| self.render_into(body, scripts);
        let env = Env { root: self.root, cache_dir: self.cache_dir, highlight: &highlight, markdown: &fragment };

        let mut out = String::new();
        let mut events: Vec<Event> = Vec::new();
        let mut ids = HeadingIds::default();
        let mut parser = Parser::new_ext(src, options).into_iter().peekable();
        let mut in_link_or_code = 0usize;
        while let Some(ev) = parser.next() {
            match ev {
                Event::Start(Tag::CodeBlock(kind)) => {
                    let mut code = String::new();
                    for inner in parser.by_ref() {
                        match inner {
                            Event::Text(t) => code.push_str(&t),
                            Event::End(TagEnd::CodeBlock) => break,
                            _ => {}
                        }
                    }
                    let mut rendered = String::new();
                    match kind {
                        CodeBlockKind::Fenced(info) => {
                            let (name, attrs) = blocks::parse_info(&info);
                            if let Some(b) = blocks::BLOCKS.iter().find(|b| b.name == name) {
                                (b.render)(&mut rendered, &code, &attrs, &env).map_err(|e| format!("```{name}: {e}"))?;
                                if let Some(s) = b.script {
                                    scripts.borrow_mut().push(s.to_string());
                                }
                            } else {
                                blocks::code(&mut rendered, &code, &name, &attrs, &env)?;
                            }
                        }
                        CodeBlockKind::Indented => blocks::code(&mut rendered, &code, "", &Attrs::new(), &env)?,
                    }
                    events.push(Event::Html(rendered.into()));
                }
                Event::Start(Tag::Heading { level, id: None, classes, attrs }) => {
                    // goldmark's auto ids, so anchors people already share keep working.
                    let mut inner = Vec::new();
                    let mut text = String::new();
                    for e in parser.by_ref() {
                        match &e {
                            Event::Text(t) | Event::Code(t) => text.push_str(t),
                            Event::End(TagEnd::Heading(_)) => break,
                            _ => {}
                        }
                        inner.push(e);
                    }
                    let id = ids.generate(&text);
                    events.push(Event::Start(Tag::Heading { level, id: Some(id.into()), classes, attrs }));
                    events.extend(inner);
                    events.push(Event::End(TagEnd::Heading(level)));
                }
                Event::Start(Tag::Link { .. }) => {
                    in_link_or_code += 1;
                    events.push(ev);
                }
                Event::End(TagEnd::Link) => {
                    in_link_or_code = in_link_or_code.saturating_sub(1);
                    events.push(ev);
                }
                Event::Text(t) if in_link_or_code == 0 && t.contains("http") => linkify(&t, &mut events),
                other => events.push(other),
            }
        }
        // Footnotes are pulldown-cmark's own: the site's CSS was written for
        // that markup (.footnote-definition).
        html::push_html(&mut out, events.into_iter());
        Ok(out)
    }
}

/// goldmark's heading ids: ASCII letters and digits lowercased, spaces,
/// dashes and underscores as dashes, everything else dropped, `-N` on a
/// repeat.
#[derive(Default)]
struct HeadingIds {
    seen: HashSet<String>,
}

impl HeadingIds {
    fn generate(&mut self, text: &str) -> String {
        let mut id = String::new();
        for c in text.trim().chars() {
            if c.is_ascii_alphanumeric() {
                id.push(c.to_ascii_lowercase());
            } else if c == ' ' || c == '\t' || c == '-' || c == '_' {
                id.push('-');
            }
        }
        if id.is_empty() {
            id = "heading".into();
        }
        if self.seen.insert(id.clone()) {
            return id;
        }
        let mut n = 1;
        loop {
            let candidate = format!("{id}-{n}");
            if self.seen.insert(candidate.clone()) {
                return candidate;
            }
            n += 1;
        }
    }
}

/// GFM autolinks for bare http(s) URLs in text, trailing punctuation left
/// out of the link.
fn linkify<'e>(text: &str, events: &mut Vec<Event<'e>>) {
    let mut rest = text;
    let mut plain = String::new();
    let scheme_at = |s: &str| s.find("http").filter(|&i| s[i..].starts_with("http://") || s[i..].starts_with("https://"));
    while let Some(i) = scheme_at(rest) {
        let starts_word = i == 0 || !rest.as_bytes()[i - 1].is_ascii_alphanumeric();
        let end = rest[i..].find(|c: char| c.is_whitespace() || c == '<').map_or(rest.len(), |e| i + e);
        let mut url = &rest[i..end];
        while let Some(last) = url.chars().last() {
            let unbalanced = last == ')' && url.matches('(').count() < url.matches(')').count();
            if ".,:;!?\"'*_~".contains(last) || unbalanced {
                url = &url[..url.len() - last.len_utf8()];
            } else {
                break;
            }
        }
        if !starts_word || url.len() <= "https://".len() {
            plain.push_str(&rest[..i + 1]);
            rest = &rest[i + 1..];
            continue;
        }
        plain.push_str(&rest[..i]);
        if !plain.is_empty() {
            events.push(Event::Text(std::mem::take(&mut plain).into()));
        }
        let dest: String = url.to_string();
        events.push(Event::Start(Tag::Link {
            link_type: pulldown_cmark::LinkType::Autolink,
            dest_url: dest.clone().into(),
            title: "".into(),
            id: "".into(),
        }));
        events.push(Event::Text(dest.into()));
        events.push(Event::End(TagEnd::Link));
        rest = &rest[i + url.len()..];
    }
    plain.push_str(rest);
    if !plain.is_empty() {
        events.push(Event::Text(plain.into()));
    }
}

