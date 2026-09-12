//! The build: content in, output tree out. This is where the page model is
//! offered to templates as objects, and where every output file is written.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::config::{self, Config};
use crate::content::{self, Site};
use crate::error::{Error, Result};
use crate::highlight::Highlighter;
use crate::markdown::Markdown;
use crate::template::{escape, Date, Host, Object, Theme, Value, Vars};

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

    let highlighter = Highlighter::new(&cfg.markdown.highlighting.theme).map_err(|m| Error::new("config.toml", m))?;
    let md = Markdown { root: &opts.root, cache_dir: &opts.cache_dir, smart_punctuation: cfg.markdown.smart_punctuation, highlighter: &highlighter };
    for pg in &mut site.pages {
        let r = md.render(&pg.body).map_err(|m| Error::new(format!("content/{}", pg.source), m))?;
        pg.content = r.html;
        pg.scripts = r.scripts;
    }
    for (name, sec) in &mut site.sections {
        let r = md.render(&sec.body).map_err(|m| Error::new(format!("section {name}"), m))?;
        sec.content = r.html;
        sec.scripts = r.scripts;
    }

    let theme = load_theme(&opts.templates)?;
    let data = Rc::new(Data { site, cfg });
    let host = SiteHost { data: data.clone() };
    let year = today().year as i64;

    check_output_dir(opts)?;
    clean(&opts.out)?;
    // Static files first, so that anything the build generates wins over a
    // file of the same name.
    copy_static(&opts.root.join("static"), &opts.out)?;
    let mut count = 0;

    // Pages: the page's own template, else the section's page_template,
    // else post.html inside a section and page.html at the root. A name
    // that is not a template is an error from render.
    for i in 0..data.site.pages.len() {
        let pg = &data.site.pages[i];
        let default = if pg.section.is_empty() { "page.html" } else { "post.html" };
        let from_section = data.site.sections.get(&pg.section).map(|s| s.page_template.as_str()).filter(|t| !t.is_empty());
        let name = if !pg.template.is_empty() { pg.template.as_str() } else { from_section.unwrap_or(default) };
        let vars = data.globals(year, &pg.url, pg.scripts.clone(), ("page", Value::object(PageRef { d: data.clone(), i })));
        let html = theme.render(name, &vars, &host).map_err(|e| e.frame(format!("page content/{}", pg.source)))?;
        write_page(&opts.out, &pg.url, &html)?;
        count += 1;
    }
    // Sections: index.html for the root, section.html for the rest.
    for (name, sec) in &data.site.sections {
        let tmpl = if !sec.template.is_empty() { sec.template.as_str() } else if name.is_empty() { "index.html" } else { "section.html" };
        let vars = data.globals(year, &sec.url, sec.scripts.clone(), ("section", Value::object(SectionRef { d: data.clone(), name: name.clone() })));
        let html = theme.render(tmpl, &vars, &host).map_err(|e| e.frame(format!("section {name:?}")))?;
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
    let syntax = highlighter.css().map_err(|m| Error::new("syntax.css", m))?;
    write(&opts.out.join("syntax.css"), syntax.as_bytes())?;

    write_feed(&data, &opts.out)?;

    Ok(Stats { pages: count, millis: start.elapsed().as_millis() })
}

/// Emptying the output directory must never empty the sources.
fn check_output_dir(opts: &Options) -> Result<()> {
    let Ok(out) = opts.out.canonicalize() else { return Ok(()) };
    for src in [opts.root.join("content"), opts.root.join("sass"), opts.root.join("static"), opts.templates.clone(), opts.root.join("config.toml")] {
        if let Ok(src) = src.canonicalize() {
            if src.starts_with(&out) {
                return Err(Error::new(opts.out.to_string_lossy(), format!("the output directory contains {}", src.display())));
            }
        }
    }
    Ok(())
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
}

impl Data {
    /// The globals plus the page-kind variable (`page`, `section` or
    /// `term`); the other two are null.
    fn globals(self: &Rc<Self>, year: i64, current_path: &str, scripts: Vec<String>, kind: (&str, Value)) -> Vars {
        let mut vars = Vars::from([
            ("config".to_string(), Value::object(ConfigRef { d: self.clone() })),
            ("site".to_string(), Value::object(SiteRef { d: self.clone() })),
            ("scripts".to_string(), Value::list(scripts.into_iter().map(Value::str).collect())),
            ("current_path".to_string(), Value::str(current_path)),
            ("year".to_string(), Value::Num(year)),
            ("page".to_string(), Value::Null),
            ("section".to_string(), Value::Null),
            ("term".to_string(), Value::Null),
        ]);
        vars.insert(kind.0.to_string(), kind.1);
        vars
    }
}

/// Front matter and config values as template values. Floats and bare times
/// were refused at load (content::check_extra), so they cannot appear.
fn toml_value(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::str(s.as_str()),
        toml::Value::Integer(n) => Value::Num(*n),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(d) => match d.date {
            Some(d) => Value::Date(Date { year: d.year as i32, month: d.month as u32, day: d.day as u32 }),
            None => Value::Null,
        },
        toml::Value::Array(a) => Value::list(a.iter().map(toml_value).collect()),
        toml::Value::Table(t) => toml_map(t),
        toml::Value::Float(_) => Value::Null,
    }
}

fn toml_map(t: &toml::Table) -> Value {
    Value::Map(Rc::new(t.iter().map(|(k, v)| (k.clone(), toml_value(v))).collect()))
}

/// Dated pages grouped by year, newest year first, each group in the
/// given order.
fn year_groups(d: &Rc<Data>, pages: &[usize]) -> Value {
    let mut groups: BTreeMap<i64, Vec<Value>> = BTreeMap::new();
    for &i in pages {
        if let Some(date) = d.site.pages[i].date {
            groups.entry(date.year as i64).or_default().push(Value::object(PageRef { d: d.clone(), i }));
        }
    }
    Value::list(groups.into_iter().rev().map(|(year, pages)| Value::object(YearGroup { year, pages })).collect())
}

struct PageRef {
    d: Rc<Data>,
    i: usize,
}

impl Object for PageRef {
    fn kind(&self) -> &'static str {
        "page"
    }
    fn field(&self, name: &str) -> Option<Value> {
        let p = &self.d.site.pages[self.i];
        let page = |i: Option<usize>| i.map_or(Value::Null, |i| Value::object(PageRef { d: self.d.clone(), i }));
        let section = |yes: bool| if yes { Value::object(SectionRef { d: self.d.clone(), name: p.section.clone() }) } else { Value::Null };
        Some(match name {
            "title" => Value::str(p.title.as_str()),
            "date" => p.date.map_or(Value::Null, Value::Date),
            "url" => Value::str(p.url.as_str()),
            "content" => Value::html(p.content.as_str()),
            "description" => p.description.as_deref().map_or(Value::Null, Value::str),
            "tags" => Value::list(
                p.tags
                    .iter()
                    .filter_map(|t| self.d.site.terms.iter().position(|term| term.name == *t))
                    .map(|i| Value::object(TermRef { d: self.d.clone(), i }))
                    .collect(),
            ),
            "extra" => toml_map(&p.extra),
            "file" => Value::str(p.source.as_str()),
            "section" => section(!p.section.is_empty()),
            "project" => section(self.d.site.sections.get(&p.section).map_or(false, |s| s.project && !s.name.is_empty())),
            "prev" => page(p.prev),
            "next" => page(p.next),
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
    fn field(&self, name: &str) -> Option<Value> {
        let s = &self.d.site.sections[&self.name];
        let pages = || s.pages.iter().map(|&i| Value::object(PageRef { d: self.d.clone(), i })).collect::<Vec<_>>();
        Some(match name {
            "name" => Value::str(s.name.as_str()),
            "title" => Value::str(s.title.as_str()),
            "description" => if s.description.is_empty() { Value::Null } else { Value::str(s.description.as_str()) },
            "url" => Value::str(s.url.as_str()),
            "content" => Value::html(s.content.as_str()),
            "project" => Value::Bool(s.project && !s.name.is_empty()),
            "extra" => toml_map(&s.extra),
            "pages" => Value::list(pages()),
            "years" => year_groups(&self.d, &s.pages),
            _ => return None,
        })
    }
}

struct YearGroup {
    year: i64,
    pages: Vec<Value>,
}

impl Object for YearGroup {
    fn kind(&self) -> &'static str {
        "year group"
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
    fn field(&self, name: &str) -> Option<Value> {
        Some(match name {
            "sections" => Value::StrictMap(Rc::new(
                self.d.site.sections.keys().map(|n| (n.clone(), Value::object(SectionRef { d: self.d.clone(), name: n.clone() }))).collect(),
            )),
            "tags" => Value::list((0..self.d.site.terms.len()).map(|i| Value::object(TermRef { d: self.d.clone(), i })).collect()),
            "projects" => Value::list(self.d.site.projects.iter().map(|n| Value::object(SectionRef { d: self.d.clone(), name: n.clone() })).collect()),
            "posts" => Value::list(self.d.site.posts.iter().map(|&i| Value::object(PageRef { d: self.d.clone(), i })).collect()),
            "years" => year_groups(&self.d, &self.d.site.posts),
            _ => return None,
        })
    }
}

/// url() for templates.
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
    write(&out.join(url.trim_matches('/')).join("index.html"), html.as_bytes())
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

fn write_feed(data: &Data, out: &Path) -> Result<()> {
    if !data.cfg.generate_feeds {
        return Ok(());
    }
    let base = data.cfg.base_url.trim_end_matches('/');
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<feed xmlns=\"http://www.w3.org/2005/Atom\">\n");
    x.push_str(&format!("  <title>{}</title>\n  <id>{base}/atom.xml</id>\n", escape(&data.cfg.title)));
    x.push_str(&format!("  <updated>{}</updated>\n", now_rfc3339()));
    x.push_str(&format!("  <link rel=\"self\" href=\"{base}/atom.xml\"/>\n  <link href=\"{base}/\"/>\n"));
    for &i in &data.site.posts {
        let pg = &data.site.pages[i];
        let updated = pg.date.map_or_else(|| "0001-01-01T00:00:00Z".to_string(), |d| format!("{d}T00:00:00Z"));
        x.push_str(&format!("  <entry>\n    <title>{}</title>\n    <id>{base}{}</id>\n", escape(&pg.title), pg.url));
        x.push_str(&format!("    <link href=\"{base}{}\"/>\n    <updated>{updated}</updated>\n", pg.url));
        x.push_str(&format!("    <content type=\"html\">{}</content>\n  </entry>\n", escape(&pg.content)));
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
