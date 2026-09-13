//! `press present TALK.md`: one Markdown file, from anywhere on disk, as a
//! talk.
//!
//!     press -site ../my-site present talk.md -out talk.html   one self-contained file
//!     press -site ../my-site present talk.md                  served, rebuilt on change
//!
//! The file is rendered like a page of the site - the same Markdown, blocks,
//! highlighting, theme and stylesheet - with the theme's `talk.html` (or the
//! template its front matter names). Then everything the page loads is folded
//! into it: stylesheets and the fonts they name, scripts, images, the icon.
//! Nothing is fetched when the file opens, so it works offline, from a USB
//! stick or a mail attachment, and a talk that cannot be published can still
//! be given without contacting a server.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::config;
use crate::content;
use crate::error::{Error, Result};
use crate::highlight::Highlighter;
use crate::markdown::Markdown;
use crate::serve;
use crate::site::{self, Options};

/// The theme template a talk renders with unless its front matter names
/// another.
const TEMPLATE: &str = "talk.html";

pub struct Rendered {
    pub html: String,
    /// The local files the page was made from, for the watcher.
    pub files: Vec<PathBuf>,
    /// References left pointing at a server, deduplicated.
    pub external: Vec<String>,
}

/// The talk as one self-contained HTML document.
pub fn render(opts: &Options, talk: &Path) -> Result<Rendered> {
    let name = talk.display().to_string();
    let cfg = config::load(&opts.root.join("config.toml"))?;
    let mut page = content::load_file(talk)?;
    let highlighter = Highlighter::new(&cfg.markdown.highlighting.theme).map_err(|m| Error::new("config.toml", m))?;
    let md = Markdown { root: &opts.root, cache_dir: &opts.cache_dir, smart_punctuation: cfg.markdown.smart_punctuation, highlighter: &highlighter };
    let r = md.render(&page.body).map_err(|m| Error::new(&name, m))?;
    page.content = r.html;
    page.scripts = r.scripts;
    let theme = site::load_theme(&opts.templates)?;
    let html = site::render_one(cfg, page, &theme, TEMPLATE)?;
    let css = lace::compile_file(opts.root.join("sass").join("main.scss"))?;

    // A root-relative reference is the site's output: the compiled main.css,
    // else a file under static/. A relative one sits next to the talk.
    let dir = talk.parent().map(Path::to_path_buf).unwrap_or_default();
    let static_dir = opts.root.join("static");
    let mut files = vec![talk.to_path_buf()];
    let (html, mut external) = {
        let mut load = |l: &Local| -> std::result::Result<Asset, String> {
            if l.rooted && l.path == "main.css" {
                return Ok(Asset { bytes: css.clone().into_bytes(), mime: "text/css" });
            }
            if l.rooted && l.path.split('/').any(|s| s == "..") {
                return Err("points outside the site's static directory".into());
            }
            let p = if l.rooted { static_dir.join(&l.path) } else { dir.join(&l.path) };
            let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let mime = mime_of(&p)?;
            files.push(p);
            Ok(Asset { bytes, mime })
        };
        let mut inliner = Inliner { load: &mut load, external: Vec::new() };
        let html = inliner.inline(&html).map_err(|m| Error::new(&name, m))?;
        (html, inliner.external)
    };
    external.sort();
    external.dedup();
    Ok(Rendered { html, files, external })
}

/// `-out FILE`: render once and write the file.
pub fn write(opts: &Options, talk: &Path, out: &Path) -> Result<()> {
    let start = Instant::now();
    let shown = out.display().to_string();
    if out.is_dir() {
        return Err(Error::new(&shown, "is a directory; -out names the HTML file to write"));
    }
    if let (Ok(a), Ok(b)) = (out.canonicalize(), talk.canonicalize()) {
        if a == b {
            return Err(Error::new(&shown, "is the talk itself"));
        }
    }
    let r = render(opts, talk)?;
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| Error::new(parent.to_string_lossy(), e.to_string()))?;
    }
    std::fs::write(out, &r.html).map_err(|e| Error::new(&shown, e.to_string()))?;
    report_external(&r.external);
    println!("{} -> {} ({} KB) in {}ms", talk.display(), shown, r.html.len().div_ceil(1024), start.elapsed().as_millis());
    Ok(())
}

/// No `-out`: serve the talk at / and rebuild it whenever the talk, a file
/// it loads, or the theme, stylesheet or static files change.
pub fn serve(opts: Options, talk: PathBuf, port: u16) -> Result<()> {
    let out = opts.cache_dir.join("present");
    std::fs::create_dir_all(&out).map_err(|e| Error::new(out.to_string_lossy(), e.to_string()))?;
    let files: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
    let opts = Arc::new(opts);
    let rebuild = {
        let (opts, talk, out, files) = (opts.clone(), talk.clone(), out.clone(), files.clone());
        move || -> Result<String> {
            let start = Instant::now();
            let r = render(&opts, &talk)?;
            let index = out.join("index.html");
            std::fs::write(&index, &r.html).map_err(|e| Error::new(index.to_string_lossy(), e.to_string()))?;
            *files.lock().unwrap() = r.files;
            report_external(&r.external);
            Ok(format!("{} -> {} KB in {}ms", talk.display(), r.html.len().div_ceil(1024), start.elapsed().as_millis()))
        }
    };
    println!("{}", rebuild()?);
    let watched = {
        let (opts, files) = (opts.clone(), files.clone());
        move || {
            let mut paths = vec![opts.templates.clone(), opts.root.join("sass"), opts.root.join("static"), opts.root.join("diagrams"), opts.root.join("config.toml")];
            paths.extend(files.lock().unwrap().iter().cloned());
            serve::fingerprint(&paths, &[])
        }
    };
    serve::run(&out, port, watched, rebuild)
}

fn report_external(external: &[String]) {
    if external.is_empty() {
        return;
    }
    eprintln!("warning: {} reference(s) still load from a server when the file opens:", external.len());
    for e in external {
        eprintln!("  {e}");
    }
}

fn mime_of(p: &Path) -> std::result::Result<&'static str, String> {
    match serve::content_type(p) {
        "application/octet-stream" => Err(format!("{}: not a kind of file a page can load inline", p.display())),
        t => Ok(t.split(';').next().unwrap_or(t).trim()),
    }
}

// --- folding a page's references into it --------------------------------------

/// A file folded into the page.
struct Asset {
    bytes: Vec<u8>,
    mime: &'static str,
}

/// A reference to a file on disk: `rooted` for /path (the site's output),
/// else relative to the talk. Query and fragment removed, %XX decoded.
struct Local {
    path: String,
    rooted: bool,
}

enum Kind {
    /// A fragment, data:, mailto: and the like: left as written.
    Keep,
    /// Would be fetched from a server: left as written, and reported.
    External,
    Local(Local),
}

fn classify(reference: &str) -> Kind {
    let r = reference.trim();
    let lower = r.to_ascii_lowercase();
    if r.is_empty() || r.starts_with('#') || ["data:", "mailto:", "tel:", "javascript:", "blob:", "about:"].iter().any(|s| lower.starts_with(s)) {
        return Kind::Keep;
    }
    if r.starts_with("//") || lower.contains("://") {
        return Kind::External;
    }
    let path = percent_decode(&r.split(['?', '#']).next().unwrap_or("").replace("&amp;", "&"));
    match path.strip_prefix('/') {
        Some(p) => Kind::Local(Local { path: p.to_string(), rooted: true }),
        None => Kind::Local(Local { path, rooted: false }),
    }
}

/// A reference found inside a stylesheet is relative to the stylesheet.
fn relative_to(base: &Local, l: Local) -> Local {
    if l.rooted {
        return l;
    }
    match base.path.rsplit_once('/') {
        Some((dir, _)) => Local { path: format!("{dir}/{}", l.path), rooted: base.rooted },
        None => Local { path: l.path, rooted: base.rooted },
    }
}

struct Inliner<'a> {
    load: &'a mut dyn FnMut(&Local) -> std::result::Result<Asset, String>,
    external: Vec<String>,
}

impl Inliner<'_> {
    /// Copies the document through, rewriting the four elements that load
    /// something: <link> (stylesheets inlined, icons as data URIs, preload,
    /// prefetch and alternate dropped), <script src> (inlined), <img src>
    /// (data URI), and <style> and inline <script> passed through untouched,
    /// so nothing inside them is mistaken for a tag.
    fn inline(&mut self, html: &str) -> std::result::Result<String, String> {
        let lower = html.to_ascii_lowercase();
        let mut out = String::with_capacity(html.len());
        let mut i = 0;
        while let Some(off) = html[i..].find('<') {
            let at = i + off;
            out.push_str(&html[i..at]);
            if lower[at..].starts_with("<!--") {
                let end = lower[at..].find("-->").map_or(html.len(), |e| at + e + 3);
                out.push_str(&html[at..end]);
                i = end;
                continue;
            }
            let name: String = lower[at + 1..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            let tag = match name.as_str() {
                "script" | "style" | "link" | "img" => Tag::parse(html, at, name.len()),
                _ => None,
            };
            let Some(tag) = tag else {
                out.push('<');
                i = at + 1;
                continue;
            };
            i = match name.as_str() {
                "script" => self.script(html, &lower, &tag, &mut out)?,
                "style" => {
                    let end = element_end(&lower, tag.end, "style");
                    out.push_str(&html[at..end]);
                    end
                }
                "link" => {
                    self.link(html, &tag, &mut out)?;
                    tag.end
                }
                _ => {
                    self.img(html, &tag, &mut out)?;
                    tag.end
                }
            };
        }
        out.push_str(&html[i..]);
        Ok(out)
    }

    fn fetch(&mut self, reference: &str, l: &Local) -> std::result::Result<Asset, String> {
        (self.load)(l).map_err(|e| format!("{reference}: {e}"))
    }

    fn script(&mut self, html: &str, lower: &str, tag: &Tag, out: &mut String) -> std::result::Result<usize, String> {
        let end = element_end(lower, tag.end, "script");
        let Some(src) = tag.get("src") else {
            out.push_str(&html[tag.start..end]);
            return Ok(end);
        };
        match classify(src) {
            Kind::Local(l) => {
                let asset = self.fetch(src, &l)?;
                let js = String::from_utf8(asset.bytes).map_err(|_| format!("{src}: not UTF-8"))?;
                let js = script_text(&js).map_err(|e| format!("{src}: {e}"))?;
                // Inline scripts run where they stand; press's templates put
                // them after the elements they use, in the order a deferred
                // load would have run them.
                let attrs: Vec<Attr> = tag.attrs.iter().filter(|a| !matches!(a.name.as_str(), "src" | "defer" | "async" | "integrity" | "crossorigin")).cloned().collect();
                write_tag(out, "script", &attrs);
                out.push_str(&js);
                out.push_str("</script>");
            }
            Kind::External => {
                self.external.push(src.to_string());
                out.push_str(&html[tag.start..end]);
            }
            Kind::Keep => out.push_str(&html[tag.start..end]),
        }
        Ok(end)
    }

    fn link(&mut self, html: &str, tag: &Tag, out: &mut String) -> std::result::Result<(), String> {
        let raw = &html[tag.start..tag.end];
        let rel = tag.get("rel").unwrap_or("").to_ascii_lowercase();
        let rels: Vec<&str> = rel.split_ascii_whitespace().collect();
        if rels.iter().any(|r| matches!(*r, "preload" | "modulepreload" | "prefetch" | "preconnect" | "dns-prefetch" | "alternate")) {
            return Ok(());
        }
        let Some(href) = tag.get("href") else {
            out.push_str(raw);
            return Ok(());
        };
        match classify(href) {
            Kind::Local(l) => {
                let asset = self.fetch(href, &l)?;
                if rels.contains(&"stylesheet") {
                    let css = String::from_utf8(asset.bytes).map_err(|_| format!("{href}: not UTF-8"))?;
                    let css = self.css(&css, &l).map_err(|e| format!("{href}: {e}"))?;
                    out.push_str("<style");
                    if let Some(media) = tag.attrs.iter().find(|a| a.name == "media") {
                        out.push(' ');
                        write_attr(out, media);
                    }
                    out.push('>');
                    out.push_str(&escape_close(&css, "style"));
                    out.push_str("</style>");
                } else {
                    let attrs = with_value(&tag.attrs, "href", data_uri(&asset));
                    write_tag(out, "link", &attrs);
                }
            }
            Kind::External => {
                self.external.push(href.to_string());
                out.push_str(raw);
            }
            Kind::Keep => out.push_str(raw),
        }
        Ok(())
    }

    fn img(&mut self, html: &str, tag: &Tag, out: &mut String) -> std::result::Result<(), String> {
        let raw = &html[tag.start..tag.end];
        let Some(src) = tag.get("src") else {
            out.push_str(raw);
            return Ok(());
        };
        match classify(src) {
            Kind::Local(l) => {
                let asset = self.fetch(src, &l)?;
                // srcset alternatives would each be another file; the one
                // src is the image the file carries.
                let attrs: Vec<Attr> = with_value(&tag.attrs, "src", data_uri(&asset)).into_iter().filter(|a| a.name != "srcset").collect();
                write_tag(out, "img", &attrs);
            }
            Kind::External => {
                self.external.push(src.to_string());
                out.push_str(raw);
            }
            Kind::Keep => out.push_str(raw),
        }
        Ok(())
    }

    /// Every url() in a stylesheet as a data URI.
    fn css(&mut self, css: &str, base: &Local) -> std::result::Result<String, String> {
        let lower = css.to_ascii_lowercase();
        let mut out = String::with_capacity(css.len());
        let mut i = 0;
        while let Some(off) = lower[i..].find("url(") {
            let open = i + off + 4;
            out.push_str(&css[i..open]);
            let vs = open + (css[open..].len() - css[open..].trim_start().len());
            let (value, after) = match css[vs..].chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let e = css[vs + 1..].find(q).ok_or("unterminated url(")?;
                    (&css[vs + 1..vs + 1 + e], vs + 2 + e)
                }
                _ => {
                    let e = css[vs..].find(')').ok_or("unterminated url(")?;
                    (css[vs..vs + e].trim_end(), vs + e)
                }
            };
            let close = after + css[after..].find(')').ok_or("unterminated url(")?;
            match classify(value) {
                Kind::Local(l) => {
                    let l = relative_to(base, l);
                    let asset = self.fetch(value, &l)?;
                    out.push('"');
                    out.push_str(&data_uri(&asset));
                    out.push('"');
                }
                Kind::External => {
                    self.external.push(value.to_string());
                    out.push_str(&css[open..close]);
                }
                Kind::Keep => out.push_str(&css[open..close]),
            }
            out.push(')');
            i = close + 1;
        }
        out.push_str(&css[i..]);
        Ok(out)
    }
}

/// A script's text, safe to place between <script> and </script>. `</script`
/// would end the element early, so its slash is escaped (`<\/script`, which
/// JavaScript reads the same in strings, templates and regular expressions).
/// `<!--` followed by `<script` would put the HTML parser into a state no
/// escape reliably gets it out of, so that is an error rather than a guess.
fn script_text(js: &str) -> std::result::Result<String, String> {
    let lower = js.to_ascii_lowercase();
    if lower.contains("<!--") && lower.contains("<script") {
        return Err("contains both \"<!--\" and \"<script\", which the HTML parser misreads inside an inline script".into());
    }
    Ok(escape_close(js, "script"))
}

/// `</name` (any case) with a backslash after the `<`.
fn escape_close(text: &str, name: &str) -> String {
    let needle = format!("</{name}");
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(off) = lower[i..].find(&needle) {
        let at = i + off;
        out.push_str(&text[i..at + 1]);
        out.push('\\');
        i = at + 1;
    }
    out.push_str(&text[i..]);
    out
}

/// The index just past `</name ...>`, searching from `from`; the end of
/// the document when there is none.
fn element_end(lower: &str, from: usize, name: &str) -> usize {
    let needle = format!("</{name}");
    match lower[from..].find(&needle) {
        Some(off) => {
            let at = from + off;
            lower[at..].find('>').map_or(lower.len(), |e| at + e + 1)
        }
        None => lower.len(),
    }
}

#[derive(Clone)]
struct Attr {
    /// Lowercased, for matching.
    name: String,
    /// As written.
    raw_name: String,
    /// As written, entities and all; None for a bare attribute.
    value: Option<String>,
    quote: char,
}

struct Tag {
    start: usize,
    /// Just past the closing `>`.
    end: usize,
    attrs: Vec<Attr>,
}

impl Tag {
    /// The start tag at `start`: `<`, a name of `name_len` bytes, attributes,
    /// `>`. None when the document ends first.
    fn parse(html: &str, start: usize, name_len: usize) -> Option<Tag> {
        let b = html.as_bytes();
        let mut p = start + 1 + name_len;
        let mut attrs = Vec::new();
        loop {
            while p < b.len() && b[p].is_ascii_whitespace() {
                p += 1;
            }
            if p >= b.len() {
                return None;
            }
            if b[p] == b'>' {
                return Some(Tag { start, end: p + 1, attrs });
            }
            if b[p] == b'/' {
                p += 1;
                continue;
            }
            let ns = p;
            while p < b.len() && !b[p].is_ascii_whitespace() && !matches!(b[p], b'=' | b'>' | b'/') {
                p += 1;
            }
            let raw_name = html[ns..p].to_string();
            let name = raw_name.to_ascii_lowercase();
            let mut q = p;
            while q < b.len() && b[q].is_ascii_whitespace() {
                q += 1;
            }
            if q >= b.len() || b[q] != b'=' {
                attrs.push(Attr { name, raw_name, value: None, quote: '"' });
                continue;
            }
            q += 1;
            while q < b.len() && b[q].is_ascii_whitespace() {
                q += 1;
            }
            if q >= b.len() {
                return None;
            }
            if b[q] == b'"' || b[q] == b'\'' {
                let quote = b[q] as char;
                let vs = q + 1;
                let ve = vs + html[vs..].find(quote)?;
                attrs.push(Attr { name, raw_name, value: Some(html[vs..ve].to_string()), quote });
                p = ve + 1;
            } else {
                let vs = q;
                let mut ve = q;
                while ve < b.len() && !b[ve].is_ascii_whitespace() && b[ve] != b'>' {
                    ve += 1;
                }
                attrs.push(Attr { name, raw_name, value: Some(html[vs..ve].to_string()), quote: '"' });
                p = ve;
            }
        }
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|a| a.name == name).and_then(|a| a.value.as_deref())
    }
}

fn with_value(attrs: &[Attr], name: &str, value: String) -> Vec<Attr> {
    attrs.iter().map(|a| if a.name == name { Attr { value: Some(value.clone()), quote: '"', ..a.clone() } } else { a.clone() }).collect()
}

fn write_attr(out: &mut String, a: &Attr) {
    out.push_str(&a.raw_name);
    if let Some(v) = &a.value {
        out.push('=');
        out.push(a.quote);
        out.push_str(v);
        out.push(a.quote);
    }
}

fn write_tag(out: &mut String, name: &str, attrs: &[Attr]) {
    out.push('<');
    out.push_str(name);
    for a in attrs {
        out.push(' ');
        write_attr(out, a);
    }
    out.push('>');
}

fn data_uri(a: &Asset) -> String {
    format!("data:{};base64,{}", a.mime, base64(&a.bytes))
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() {
            let hex = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
            out.push(hex(b[i + 1]) * 16 + hex(b[i + 2]));
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Inlines `html` against an in-memory file set keyed "/rooted" or
    /// "relative".
    fn inline(html: &str, files: &[(&str, &str, &'static str)]) -> std::result::Result<(String, Vec<String>), String> {
        let map: HashMap<String, (Vec<u8>, &'static str)> = files.iter().map(|(p, b, m)| (p.to_string(), (b.as_bytes().to_vec(), *m))).collect();
        let mut load = |l: &Local| -> std::result::Result<Asset, String> {
            let key = if l.rooted { format!("/{}", l.path) } else { l.path.clone() };
            map.get(&key).map(|(b, m)| Asset { bytes: b.clone(), mime: m }).ok_or_else(|| format!("no such file {key}"))
        };
        let mut inliner = Inliner { load: &mut load, external: Vec::new() };
        let out = inliner.inline(html)?;
        Ok((out, inliner.external))
    }

    #[test]
    fn base64_vectors() {
        for (input, want) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(input.as_bytes()), want);
        }
    }

    #[test]
    fn scripts_are_inlined_and_cannot_end_early() {
        let (out, _) = inline(
            r#"<p>x</p><script src="/js/a.js" defer></script><script>var s = "<img src=/nope.png>";</script>"#,
            &[("/js/a.js", "var t = '</SCRIPT>';", "text/javascript")],
        )
        .unwrap();
        assert_eq!(out, r#"<p>x</p><script>var t = '<\/SCRIPT>';</script><script>var s = "<img src=/nope.png>";</script>"#);
    }

    #[test]
    fn a_script_the_parser_would_misread_is_an_error() {
        let err = inline(r#"<script src="/bad.js"></script>"#, &[("/bad.js", "a('<!--'); b('<script>');", "text/javascript")]).unwrap_err();
        assert!(err.contains("/bad.js"), "{err}");
    }

    #[test]
    fn stylesheets_become_style_with_their_urls_inlined() {
        let (out, _) = inline(
            r#"<link rel="preload" as="font" href="/f.woff2" crossorigin><link rel="stylesheet" href="/main.css"><link rel="alternate" href="/atom.xml">"#,
            &[("/main.css", r#"@font-face{src:url("/f.woff2")} a{background:url( data:image/png;base64,AA )}"#, "text/css"), ("/f.woff2", "foo", "font/woff2")],
        )
        .unwrap();
        assert_eq!(out, r#"<style>@font-face{src:url("data:font/woff2;base64,Zm9v")} a{background:url( data:image/png;base64,AA )}</style>"#);
    }

    #[test]
    fn a_stylesheet_next_to_the_talk_resolves_its_urls_next_to_itself() {
        let (out, _) = inline(r#"<link rel="stylesheet" href="css/talk.css">"#, &[("css/talk.css", "b{background:url(bg.png)}", "text/css"), ("css/bg.png", "foo", "image/png")]).unwrap();
        assert_eq!(out, r#"<style>b{background:url("data:image/png;base64,Zm9v")}</style>"#);
    }

    #[test]
    fn icons_and_images_become_data_uris_and_links_stay() {
        let (out, ext) = inline(
            r#"<link rel="icon" href="/favicon.ico" type="image/x-icon"><img src="my%20photo.webp" alt='a "b"'><img src="https://example.com/x.png"><a href="/elsewhere/">l</a>"#,
            &[("/favicon.ico", "foo", "image/x-icon"), ("my photo.webp", "foo", "image/webp")],
        )
        .unwrap();
        assert_eq!(
            out,
            r#"<link rel="icon" href="data:image/x-icon;base64,Zm9v" type="image/x-icon"><img src="data:image/webp;base64,Zm9v" alt='a "b"'><img src="https://example.com/x.png"><a href="/elsewhere/">l</a>"#
        );
        assert_eq!(ext, vec!["https://example.com/x.png".to_string()]);
    }

    #[test]
    fn comments_and_style_elements_pass_through() {
        let html = r#"<!-- <img src="gone.png"> --><style>a::after{content:"<img src=x>"}</style>"#;
        assert_eq!(inline(html, &[]).unwrap().0, html);
    }

    #[test]
    fn a_missing_file_is_an_error_naming_it() {
        let err = inline(r#"<img src="gone.png">"#, &[]).unwrap_err();
        assert!(err.contains("gone.png"), "{err}");
    }
}
