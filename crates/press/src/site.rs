//! The build: content in, output tree out. This is where the page model is
//! offered to templates as objects, and where every output file is written.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use sha2::{Digest, Sha384};

use crate::config::{self, Config};
use crate::content::{self, extra_str, Site};
use crate::error::{Error, Result};
use crate::highlight::Highlighter;
use crate::markdown::Markdown;
use crate::template::{Date, Host, Object, Theme, Value, Vars};

pub struct Options {
    pub root: PathBuf,
    pub out: PathBuf,
    pub templates: PathBuf,
    pub cache_dir: PathBuf,
}

pub struct Stats {
    pub pages: usize,
    pub millis: u128,
}

pub fn build(opts: &Options) -> Result<Stats> {
    let start = Instant::now();
    let cfg = config::load(&opts.root.join("config.toml"))?;
    let mut site = content::load(&opts.root)?;

    let highlighter = Highlighter::new(&cfg.markdown.highlighting.theme);
    let md = Markdown {
        root: &opts.root,
        cache_dir: Some(&opts.cache_dir),
        smart_punctuation: cfg.markdown.smart_punctuation,
        highlighter: &highlighter,
    };
    for pg in &mut site.pages {
        let r = md.render(&pg.body).map_err(|m| Error::new(format!("content/{}", pg.source), m))?;
        pg.content = r.html;
        pg.scripts = r.scripts;
    }
    let mut section_scripts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, sec) in &mut site.sections {
        let r = md.render(&sec.body).map_err(|m| Error::new(format!("section {name}"), m))?;
        sec.content = r.html;
        section_scripts.insert(name.clone(), r.scripts);
    }

    let theme = load_theme(&opts.templates)?;
    let data = Rc::new(Data::new(site, cfg));
    let host = SiteHost { data: data.clone() };
    let year = today().year as i64;

    clean(&opts.out)?;
    let mut count = 0;

    // Pages.
    for i in 0..data.site.pages.len() {
        let pg = &data.site.pages[i];
        let section = &data.site.sections[&pg.section];
        let name = if !section.page_template.is_empty() && theme.has(&section.page_template) {
            section.page_template.clone()
        } else {
            "page.html".to_string()
        };
        let vars = data.globals(year, &pg.url, pg.scripts.clone(), ("page", Value::object(PageRef { d: data.clone(), i })));
        let html = theme.render(&name, &vars, &host).map_err(|e| e.frame(format!("page content/{}", pg.source)))?;
        write_page(&opts.out, &pg.url, &html)?;
        count += 1;
    }
    // Sections.
    for name in data.site.sections.keys() {
        let sec = &data.site.sections[name];
        let tmpl = if !sec.template.is_empty() && theme.has(&sec.template) { sec.template.clone() } else { "index.html".to_string() };
        let scripts = section_scripts.get(name).cloned().unwrap_or_default();
        let vars = data.globals(year, &sec.url, scripts, ("section", Value::object(SectionRef { d: data.clone(), name: name.clone() })));
        let html = theme.render(&tmpl, &vars, &host).map_err(|e| e.frame(format!("section {name:?}")))?;
        write_page(&opts.out, &sec.url, &html)?;
        count += 1;
    }
    // Tags.
    let vars = data.globals(year, "/tags/", Vec::new(), ("term", Value::Null));
    let html = theme.render("tags-list.html", &vars, &host)?;
    write_page(&opts.out, "/tags/", &html)?;
    count += 1;
    for i in 0..data.site.terms.len() {
        let term = &data.site.terms[i];
        let vars = data.globals(year, &term.url, Vec::new(), ("term", Value::object(TermRef { d: data.clone(), i })));
        let html = theme.render("tags-single.html", &vars, &host).map_err(|e| e.frame(format!("tag {:?}", term.name)))?;
        write_page(&opts.out, &term.url, &html)?;
        count += 1;
    }

    // CSS.
    let css = lace::compile_file(opts.root.join("sass").join("main.scss"))?;
    write(&opts.out.join("main.css"), css.as_bytes())?;
    write(&opts.out.join("syntax.css"), highlighter.css().as_bytes())?;

    copy_static(&opts.root.join("static"), &opts.out)?;
    write_feed(&data, &opts.out)?;

    Ok(Stats { pages: count, millis: start.elapsed().as_millis() })
}

fn load_theme(dir: &Path) -> Result<Theme> {
    let mut files = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| Error::new(dir.to_string_lossy(), e.to_string()))?;
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().map_or(false, |x| x == "html") {
            let name = format!("theme/{}", p.file_name().unwrap().to_string_lossy());
            let src = std::fs::read_to_string(&p).map_err(|err| Error::new(&name, err.to_string()))?;
            files.push((name, src));
        }
    }
    if files.is_empty() {
        return Err(Error::new(dir.to_string_lossy(), "no templates"));
    }
    Theme::load(&files)
}

// --- the page model as template objects ---------------------------------------

/// Everything the templates can reach, immutable once the build starts.
struct Data {
    site: Site,
    cfg: Config,
    term_index: HashMap<String, usize>,
}

impl Data {
    fn new(site: Site, cfg: Config) -> Data {
        let term_index = site.terms.iter().enumerate().map(|(i, t)| (t.name.clone(), i)).collect();
        Data { site, cfg, term_index }
    }
}

trait Globals {
    fn globals(&self, year: i64, current_path: &str, scripts: Vec<String>, kind: (&str, Value)) -> Vars;
}

impl Globals for Rc<Data> {
    fn globals(&self, year: i64, current_path: &str, scripts: Vec<String>, kind: (&str, Value)) -> Vars {
        let mut vars: Vars = vec![
            ("config".into(), Value::object(ConfigRef { d: self.clone() })),
            ("site".into(), Value::object(SiteRef { d: self.clone() })),
            ("scripts".into(), Value::list(scripts.into_iter().map(Value::str).collect())),
            ("current_path".into(), Value::str(current_path)),
            ("year".into(), Value::Num(year)),
            ("page".into(), Value::Null),
            ("section".into(), Value::Null),
            ("term".into(), Value::Null),
        ];
        let slot = vars.iter_mut().find(|(n, _)| n == kind.0).unwrap();
        slot.1 = kind.1;
        vars
    }
}

fn toml_value(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::str(s.as_str()),
        toml::Value::Integer(n) => Value::Num(*n),
        toml::Value::Float(f) => Value::Num(*f as i64),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(d) => match d.date {
            Some(d) => Value::Date(Date { year: d.year as i32, month: d.month as u32, day: d.day as u32 }),
            None => Value::Null,
        },
        toml::Value::Array(a) => Value::list(a.iter().map(toml_value).collect()),
        toml::Value::Table(t) => toml_map(t),
    }
}

fn toml_map(t: &toml::Table) -> Value {
    Value::Map(Rc::new(t.iter().map(|(k, v)| (k.clone(), toml_value(v))).collect()))
}

struct PageRef {
    d: Rc<Data>,
    i: usize,
}

impl Object for PageRef {
    fn kind(&self) -> &'static str {
        "page"
    }
    fn ident(&self) -> usize {
        self.i
    }
    fn field(&self, name: &str) -> Option<Value> {
        let p = &self.d.site.pages[self.i];
        let page = |i: Option<usize>| i.map_or(Value::Null, |i| Value::object(PageRef { d: self.d.clone(), i }));
        Some(match name {
            "title" => Value::str(p.title.as_str()),
            "date" => p.date.map_or(Value::Null, Value::Date),
            "url" => Value::str(p.url.as_str()),
            "content" => Value::html(p.content.as_str()),
            "description" => p.description.as_deref().map_or(Value::Null, Value::str),
            "tags" => Value::list(
                p.tags.iter().filter_map(|t| self.d.term_index.get(t)).map(|&i| Value::object(TermRef { d: self.d.clone(), i })).collect(),
            ),
            "extra" => toml_map(&p.extra),
            "section" => Value::str(p.section.as_str()),
            "earlier" => page(p.earlier),
            "later" => page(p.later),
            "project" => page(self.d.site.owner_of(self.i)),
            "posts" => Value::list(self.d.site.posts_for(extra_str(&p.extra, "slug")).into_iter().map(|i| page(Some(i))).collect()),
            _ => return None,
        })
    }
}

struct SectionRef {
    d: Rc<Data>,
    name: String,
}

impl Object for SectionRef {
    fn kind(&self) -> &'static str {
        "section"
    }
    fn ident(&self) -> usize {
        self.d.site.sections.keys().position(|k| *k == self.name).unwrap_or(0)
    }
    fn field(&self, name: &str) -> Option<Value> {
        let s = &self.d.site.sections[&self.name];
        let pages = || s.pages.iter().map(|&i| Value::object(PageRef { d: self.d.clone(), i })).collect::<Vec<_>>();
        Some(match name {
            "name" => Value::str(s.name.as_str()),
            "title" => Value::str(s.title.as_str()),
            "url" => Value::str(s.url.as_str()),
            "content" => Value::html(s.content.as_str()),
            "pages" => Value::list(pages()),
            "years" => {
                // Consecutive runs of the same year, in the section's order.
                let mut groups: Vec<(i64, Vec<Value>)> = Vec::new();
                for &i in &s.pages {
                    let Some(d) = self.d.site.pages[i].date else { continue };
                    let y = d.year as i64;
                    match groups.last_mut() {
                        Some((gy, ps)) if *gy == y => ps.push(Value::object(PageRef { d: self.d.clone(), i })),
                        _ => groups.push((y, vec![Value::object(PageRef { d: self.d.clone(), i })])),
                    }
                }
                Value::list(groups.into_iter().enumerate().map(|(k, (y, ps))| Value::object(YearGroup { year: y, pages: ps, ident: k })).collect())
            }
            _ => return None,
        })
    }
}

struct YearGroup {
    year: i64,
    pages: Vec<Value>,
    ident: usize,
}

impl Object for YearGroup {
    fn kind(&self) -> &'static str {
        "year group"
    }
    fn ident(&self) -> usize {
        self.ident
    }
    fn field(&self, name: &str) -> Option<Value> {
        Some(match name {
            "year" => Value::Num(self.year),
            "pages" => Value::list(self.pages.clone()),
            _ => return None,
        })
    }
}

struct TermRef {
    d: Rc<Data>,
    i: usize,
}

impl Object for TermRef {
    fn kind(&self) -> &'static str {
        "term"
    }
    fn ident(&self) -> usize {
        self.i
    }
    fn field(&self, name: &str) -> Option<Value> {
        let t = &self.d.site.terms[self.i];
        Some(match name {
            "name" => Value::str(t.name.as_str()),
            "url" => Value::str(t.url.as_str()),
            "pages" => Value::list(t.pages.iter().map(|&i| Value::object(PageRef { d: self.d.clone(), i })).collect()),
            _ => return None,
        })
    }
}

struct ConfigRef {
    d: Rc<Data>,
}

impl Object for ConfigRef {
    fn kind(&self) -> &'static str {
        "config"
    }
    fn ident(&self) -> usize {
        0
    }
    fn field(&self, name: &str) -> Option<Value> {
        let c = &self.d.cfg;
        Some(match name {
            "base_url" => Value::str(c.base_url.as_str()),
            "title" => Value::str(c.title.as_str()),
            "description" => Value::str(c.description.as_str()),
            "default_language" => Value::str(c.default_language.as_str()),
            "extra" => toml_map(&c.extra),
            _ => return None,
        })
    }
}

struct SiteRef {
    d: Rc<Data>,
}

impl Object for SiteRef {
    fn kind(&self) -> &'static str {
        "site"
    }
    fn ident(&self) -> usize {
        0
    }
    fn field(&self, name: &str) -> Option<Value> {
        Some(match name {
            "sections" => Value::StrictMap(Rc::new(
                self.d.site.sections.keys().map(|n| (n.clone(), Value::object(SectionRef { d: self.d.clone(), name: n.clone() }))).collect(),
            )),
            "tags" => Value::list((0..self.d.site.terms.len()).map(|i| Value::object(TermRef { d: self.d.clone(), i })).collect()),
            _ => return None,
        })
    }
}

/// url() and sri() for templates.
struct SiteHost {
    data: Rc<Data>,
}

impl Host for SiteHost {
    /// Root-relative on purpose: the same build then works unchanged from
    /// localhost or from the live domain.
    fn url(&self, path: &str) -> std::result::Result<String, String> {
        if path.starts_with("@/") {
            return self.data.site.resolve(path).ok_or_else(|| format!("no content file {path:?}"));
        }
        Ok(format!("/{}", path.trim_start_matches('/')))
    }

    /// A subresource integrity digest for a vendored file, so a tampered
    /// script is refused by the browser.
    fn sri(&self, path: &str) -> std::result::Result<String, String> {
        let p = self.data.site.root.join("static").join(path.trim_start_matches('/'));
        let bytes = std::fs::read(&p).map_err(|e| format!("sri({path:?}): {e}"))?;
        let digest = Sha384::digest(&bytes);
        Ok(format!("sha384-{}", base64::engine::general_purpose::STANDARD.encode(digest)))
    }
}

// --- output -----------------------------------------------------------------

/// Empties the output tree without removing the directory itself: removing
/// it outright fails whenever anything holds a handle on it, and on Windows
/// that is easy to do by accident.
pub fn clean(dir: &Path) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(Error::new(dir.to_string_lossy(), e.to_string())),
    };
    for e in entries.flatten() {
        let p = e.path();
        let r = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        r.map_err(|err| Error::new(p.to_string_lossy(), err.to_string()))?;
    }
    Ok(())
}

/// A rendered page at its URL: /foo/ becomes foo/index.html.
fn write_page(out: &Path, url: &str, html: &str) -> Result<()> {
    let mut p = out.to_path_buf();
    for seg in url.trim_matches('/').split('/').filter(|s| !s.is_empty()) {
        p.push(seg);
    }
    write(&p.join("index.html"), html.as_bytes())
}

fn write(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::new(dir.to_string_lossy(), e.to_string()))?;
    }
    std::fs::write(path, data).map_err(|e| Error::new(path.to_string_lossy(), e.to_string()))
}

fn copy_static(from: &Path, to: &Path) -> Result<()> {
    let entries = match std::fs::read_dir(from) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(Error::new(from.to_string_lossy(), e.to_string())),
    };
    for e in entries.flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_static(&src, &dst)?;
        } else {
            if let Some(dir) = dst.parent() {
                std::fs::create_dir_all(dir).map_err(|err| Error::new(dir.to_string_lossy(), err.to_string()))?;
            }
            std::fs::copy(&src, &dst).map_err(|err| Error::new(src.to_string_lossy(), err.to_string()))?;
        }
    }
    Ok(())
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn write_feed(data: &Data, out: &Path) -> Result<()> {
    if !data.cfg.generate_feeds {
        return Ok(());
    }
    let Some(sec) = data.site.sections.get("writing") else { return Ok(()) };
    let base = data.cfg.base_url.trim_end_matches('/');
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<feed xmlns=\"http://www.w3.org/2005/Atom\">\n");
    x.push_str(&format!("  <title>{}</title>\n  <id>{base}/atom.xml</id>\n", xml_escape(&data.cfg.title)));
    x.push_str(&format!("  <updated>{}</updated>\n", now_rfc3339()));
    x.push_str(&format!("  <link rel=\"self\" href=\"{base}/atom.xml\"/>\n  <link href=\"{base}/\"/>\n"));
    for &i in &sec.pages {
        let pg = &data.site.pages[i];
        let updated = pg.date.map_or_else(|| "0001-01-01T00:00:00Z".to_string(), |d| format!("{d}T00:00:00Z"));
        x.push_str(&format!("  <entry>\n    <title>{}</title>\n    <id>{base}{}</id>\n", xml_escape(&pg.title), pg.url));
        x.push_str(&format!("    <link href=\"{base}{}\"/>\n    <updated>{updated}</updated>\n", pg.url));
        x.push_str(&format!("    <content type=\"html\">{}</content>\n  </entry>\n", xml_escape(&pg.content)));
    }
    x.push_str("</feed>\n");
    write(&out.join("atom.xml"), x.as_bytes())
}

// --- time, without a dependency ---------------------------------------------

/// Days since the epoch to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> Date {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    Date { year: (if m <= 2 { y + 1 } else { y }) as i32, month: m as u32, day: d as u32 }
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn today() -> Date {
    civil_from_days(now_secs().div_euclid(86400))
}

fn now_rfc3339() -> String {
    let s = now_secs();
    let d = civil_from_days(s.div_euclid(86400));
    let t = s.rem_euclid(86400);
    format!("{d}T{:02}:{:02}:{:02}Z", t / 3600, t % 3600 / 60, t % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0).to_string(), "1970-01-01");
        assert_eq!(civil_from_days(19965).to_string(), "2024-08-30");
        assert_eq!(civil_from_days(20695).to_string(), "2026-08-30");
    }
}
