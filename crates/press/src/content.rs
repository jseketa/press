//! The content tree as a page model: front matter, URLs, sections and
//! taxonomies. A directory with an info.md is a section - a project, unless
//! its info.md says `project = false` - and the other .md files in it are
//! its pages, in file-name order unless it says otherwise; .md files at the
//! root are pages of the root section. It knows nothing about rendering.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::template::Date;

#[derive(Deserialize, Default)]
#[serde(default)]
struct FrontMatter {
    title: String,
    description: String,
    date: Option<toml::value::Datetime>,
    weight: i64,
    path: String,
    sort_by: String,
    template: String,
    page_template: String,
    /// On an info.md: `project = false` makes a folder of posts that is not
    /// a project (the site's `unsorted`).
    project: Option<bool>,
    taxonomies: BTreeMap<String, Vec<String>>,
    extra: toml::Table,
}

pub struct Page {
    pub title: String,
    pub description: Option<String>,
    pub date: Option<Date>,
    pub weight: i64,
    pub tags: Vec<String>,
    pub extra: toml::Table,
    pub body: String,
    /// Rendered HTML and the client-side libraries its blocks need, filled
    /// in by the build.
    pub content: String,
    pub scripts: Vec<String>,
    /// Root-relative, e.g. /swd-protocol/.
    pub url: String,
    /// The section's name: the directory, "" for the root.
    pub section: String,
    /// `template = "x.html"` in the front matter; "" for the default.
    pub template: String,
    /// Neighbours in the site-wide chronological order of posts.
    pub earlier: Option<usize>,
    pub later: Option<usize>,
    /// Path under content/, with forward slashes.
    pub source: String,
}

pub struct Section {
    pub name: String,
    pub title: String,
    pub description: String,
    pub weight: i64,
    pub project: bool,
    pub sort_by: String,
    pub template: String,
    pub page_template: String,
    pub extra: toml::Table,
    pub body: String,
    pub content: String,
    pub scripts: Vec<String>,
    pub pages: Vec<usize>,
    pub url: String,
}

pub struct Term {
    pub name: String,
    pub url: String,
    pub pages: Vec<usize>,
}

pub struct Site {
    pub sections: BTreeMap<String, Section>,
    pub pages: Vec<Page>,
    pub terms: Vec<Term>,
    /// Every page inside a section, newest first.
    pub posts: Vec<usize>,
    /// The project sections, by weight then name.
    pub projects: Vec<String>,
}

/// Splits the TOML block between +++ fences from the body. Line endings
/// may be CRLF; the fence itself is `+++` at the start of a line. Every
/// page has one: a file without is more likely a mistake than a page.
fn split_front_matter(src: &str) -> std::result::Result<(FrontMatter, String), String> {
    let s = src.strip_prefix('\u{feff}').unwrap_or(src).trim_start_matches([' ', '\t', '\r', '\n']);
    let Some(rest) = s.strip_prefix("+++") else {
        return Err("no +++ front matter".into());
    };
    let Some(end) = rest.find("\n+++") else {
        return Err("unterminated +++ front matter".into());
    };
    let fm: FrontMatter = toml::from_str(rest[..end].trim_end_matches('\r')).map_err(|e| e.to_string())?;
    Ok((fm, rest[end + 4..].trim_start_matches(['\r', '\n']).to_string()))
}

/// A file name without its ordering prefix: `02-swd-protocol` is
/// `swd-protocol` on the web.
fn unnumbered(name: &str) -> &str {
    let digits = name.bytes().take_while(u8::is_ascii_digit).count();
    match name[digits..].strip_prefix('-') {
        Some(rest) if digits > 0 && !rest.is_empty() => rest,
        _ => name,
    }
}

/// The URL for a file under content/: /dir/ for a section, /dir/name/ for
/// a page in it, /name/ at the root. The `path` override keeps the
/// historical root-level post URLs alive; those are live links, not
/// derived from where the file sits.
fn page_url(rel: &str, fm: &FrontMatter) -> std::result::Result<String, String> {
    if !fm.path.is_empty() {
        let p = fm.path.trim_matches('/');
        if p.is_empty() || p.contains('\\') || p.split('/').any(|s| s == "..") {
            return Err(format!("path = {:?} is not a URL path", fm.path));
        }
        return Ok(format!("/{p}/"));
    }
    let rel = rel.strip_suffix(".md").unwrap_or(rel);
    if rel == "info" {
        return Ok("/".into());
    }
    if let Some(dir) = rel.strip_suffix("/info") {
        return Ok(format!("/{dir}/"));
    }
    match rel.rsplit_once('/') {
        Some((dir, file)) => Ok(format!("/{dir}/{}/", unnumbered(file))),
        None => Ok(format!("/{}/", unnumbered(rel))),
    }
}

pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out)?;
        } else if p.extension().map_or(false, |x| x == "md") {
            out.push(p);
        }
    }
    Ok(())
}

pub fn load(root: &Path) -> Result<Site> {
    let dir = root.join("content");
    let mut files = Vec::new();
    walk(&dir, &mut files).map_err(|e| Error::new("content", e.to_string()))?;

    let mut site = Site { sections: BTreeMap::new(), pages: Vec::new(), terms: Vec::new(), posts: Vec::new(), projects: Vec::new() };
    for p in files {
        let rel = p.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/");
        let src = format!("content/{rel}");
        let raw = std::fs::read_to_string(&p).map_err(|e| Error::new(&src, e.to_string()))?;
        let (fm, body) = split_front_matter(&raw).map_err(|m| Error::new(&src, m))?;
        let date = match &fm.date {
            Some(dt) => match dt.date {
                Some(d) => Some(Date { year: d.year as i32, month: d.month as u32, day: d.day as u32 }),
                None => return Err(Error::new(&src, "date must include a calendar day")),
            },
            None => None,
        };
        check_extra(&fm.extra, &src)?;
        let url = page_url(&rel, &fm).map_err(|m| Error::new(&src, m))?;
        if rel == "info.md" || rel.ends_with("/info.md") {
            let name = rel.strip_suffix("info.md").unwrap().trim_end_matches('/').to_string();
            site.sections.insert(
                name.clone(),
                Section {
                    name,
                    title: fm.title,
                    description: fm.description,
                    weight: fm.weight,
                    project: fm.project.unwrap_or(true),
                    sort_by: fm.sort_by,
                    template: fm.template,
                    page_template: fm.page_template,
                    extra: fm.extra,
                    body,
                    content: String::new(),
                    scripts: Vec::new(),
                    pages: Vec::new(),
                    url,
                },
            );
            continue;
        }
        let section = match rel.rfind('/') {
            Some(i) => rel[..i].to_string(),
            None => String::new(),
        };
        site.pages.push(Page {
            title: fm.title,
            description: if fm.description.is_empty() { None } else { Some(fm.description) },
            date,
            weight: fm.weight,
            tags: fm.taxonomies.get("tags").cloned().unwrap_or_default(),
            extra: fm.extra,
            body,
            content: String::new(),
            scripts: Vec::new(),
            url,
            section,
            template: fm.template,
            earlier: None,
            later: None,
            source: rel,
        });
    }
    site.attach()?;
    site.build_terms()?;
    site.check_urls()?;
    Ok(site)
}

/// Template numbers are integers and dates are calendar days; a float or a
/// bare time in front matter would print as something the author did not
/// write, so both are refused up front. `file` names the source in errors.
pub fn check_extra(t: &toml::Table, file: &str) -> Result<()> {
    fn check(v: &toml::Value, key: &str, file: &str) -> Result<()> {
        match v {
            toml::Value::Float(_) => Err(Error::new(file, format!("{key}: floats are not supported, use an integer or a string"))),
            toml::Value::Datetime(d) if d.date.is_none() => Err(Error::new(file, format!("{key}: a time without a date is not supported"))),
            toml::Value::Array(a) => a.iter().try_for_each(|v| check(v, key, file)),
            toml::Value::Table(t) => t.iter().try_for_each(|(k, v)| check(v, &format!("{key}.{k}"), file)),
            _ => Ok(()),
        }
    }
    t.iter().try_for_each(|(k, v)| check(v, &format!("extra.{k}"), file))
}

impl Site {
    /// Pages into their sections (a page's directory must have an info.md),
    /// sections into their order, posts into the site-wide chronology.
    fn attach(&mut self) -> Result<()> {
        for (i, pg) in self.pages.iter().enumerate() {
            match self.sections.get_mut(&pg.section) {
                Some(sec) => sec.pages.push(i),
                None => return Err(Error::new(format!("content/{}", pg.source), format!("the folder {:?} has no info.md", pg.section))),
            }
        }
        for sec in self.sections.values_mut() {
            match sec.sort_by.as_str() {
                "" | "name" => {}
                "weight" => sec.pages.sort_by_key(|&i| self.pages[i].weight),
                "date" => sec.pages.sort_by(|&a, &b| self.pages[b].date.cmp(&self.pages[a].date)),
                other => return Err(Error::new(format!("content/{}info.md", prefix(&sec.name)), format!("sort_by = {other:?}; name, weight or date"))),
            }
        }
        // Every page inside a section is a post; newest first, undated last.
        self.posts = (0..self.pages.len()).filter(|&i| !self.pages[i].section.is_empty()).collect();
        self.posts.sort_by(|&a, &b| self.pages[b].date.cmp(&self.pages[a].date).then_with(|| self.pages[a].source.cmp(&self.pages[b].source)));
        for (k, &i) in self.posts.iter().enumerate() {
            self.pages[i].earlier = self.posts.get(k + 1).copied();
            self.pages[i].later = if k > 0 { Some(self.posts[k - 1]) } else { None };
        }
        self.projects = self.sections.values().filter(|s| !s.name.is_empty() && s.project).map(|s| s.name.clone()).collect();
        self.projects.sort_by_key(|n| (self.sections[n].weight, n.clone()));
        Ok(())
    }

    fn build_terms(&mut self) -> Result<()> {
        let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, pg) in self.pages.iter().enumerate() {
            for t in &pg.tags {
                by_name.entry(t.clone()).or_default().push(i);
            }
        }
        for (name, mut pages) in by_name {
            pages.sort_by(|&a, &b| self.pages[b].date.cmp(&self.pages[a].date));
            let slug = slugify(&name);
            if slug.is_empty() {
                return Err(Error::new("content", format!("tag {name:?} has no URL-safe characters")));
            }
            let url = format!("/tags/{slug}/");
            if let Some(other) = self.terms.iter().find(|t| t.url == url) {
                return Err(Error::new("content", format!("tags {:?} and {name:?} share the URL {url}", other.name)));
            }
            self.terms.push(Term { name, url, pages });
        }
        Ok(())
    }

    /// Two things at one URL would silently overwrite each other.
    fn check_urls(&self) -> Result<()> {
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
        let sections = self.sections.values().map(|s| (s.url.as_str(), format!("content/{}info.md", prefix(&s.name))));
        let pages = self.pages.iter().map(|p| (p.url.as_str(), format!("content/{}", p.source)));
        let terms = self.terms.iter().map(|t| (t.url.as_str(), format!("tag {:?}", t.name)));
        let mut names: Vec<(&str, String)> = sections.chain(pages).chain(terms).collect();
        names.push(("/tags/", "the tag index".into()));
        for (url, what) in &names {
            if let Some(other) = seen.insert(url, what) {
                return Err(Error::new("content", format!("{other} and {what} share the URL {url}")));
            }
        }
        Ok(())
    }

    /// `@/path.md` as a URL: `@/dir/info.md` is the section, `@/info.md`
    /// the home, anything else a page by its path under content/.
    pub fn resolve(&self, reference: &str) -> Option<String> {
        let rel = reference.strip_prefix("@/").unwrap_or(reference);
        if let Some(dir) = rel.strip_suffix("info.md") {
            return self.sections.get(dir.trim_end_matches('/')).map(|s| s.url.clone());
        }
        self.pages.iter().find(|p| p.source == rel).map(|p| p.url.clone())
    }
}

/// `dir/` for a section name, "" for the root: the path its info.md sits under.
fn prefix(name: &str) -> String {
    if name.is_empty() { String::new() } else { format!("{name}/") }
}
