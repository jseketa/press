//! The content tree as a page model: front matter, URLs, sections and
//! taxonomies. It knows nothing about rendering.

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
    /// The section's name; "" for the root.
    pub section: String,
    pub earlier: Option<usize>,
    pub later: Option<usize>,
    /// Path under content/, with forward slashes.
    pub source: String,
}

/// A section's description and extra are parsed but not offered to templates
/// until a theme reads them.
pub struct Section {
    pub name: String,
    pub title: String,
    pub sort_by: String,
    pub template: String,
    pub page_template: String,
    pub body: String,
    pub content: String,
    pub pages: Vec<usize>,
    pub url: String,
}

pub struct Term {
    pub name: String,
    pub url: String,
    pub pages: Vec<usize>,
}

pub struct Site {
    pub root: PathBuf,
    pub sections: BTreeMap<String, Section>,
    pub pages: Vec<Page>,
    pub terms: Vec<Term>,
}

/// Splits the TOML block between +++ fences from the body.
fn split_front_matter(src: &str) -> std::result::Result<(FrontMatter, String), String> {
    let s = src.strip_prefix('\u{feff}').unwrap_or(src).trim_start_matches([' ', '\t', '\r', '\n']);
    let Some(rest) = s.strip_prefix("+++") else {
        return Ok((FrontMatter::default(), s.to_string()));
    };
    let Some(end) = rest.find("\n+++") else {
        return Err("unterminated +++ front matter".into());
    };
    let fm: FrontMatter = toml::from_str(&rest[..end]).map_err(|e| e.message().to_string())?;
    Ok((fm, rest[end + 4..].trim_start_matches(['\r', '\n']).to_string()))
}

/// Zola's URL rules, including the `path` override that keeps the historical
/// root-level post URLs alive. Those are live links; they are not derived
/// from where the file sits.
fn page_url(rel: &str, fm: &FrontMatter) -> String {
    if !fm.path.is_empty() {
        return format!("/{}/", fm.path.trim_matches('/'));
    }
    let rel = rel.strip_suffix(".md").unwrap_or(rel);
    if rel == "_index" {
        return "/".into();
    }
    if let Some(dir) = rel.strip_suffix("/_index") {
        return format!("/{dir}/");
    }
    format!("/{rel}/")
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

    let mut site = Site { root: root.to_path_buf(), sections: BTreeMap::new(), pages: Vec::new(), terms: Vec::new() };
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
        if rel.ends_with("_index.md") {
            let name = rel.strip_suffix("_index.md").unwrap().trim_end_matches('/').to_string();
            let url = page_url(&rel, &fm);
            site.sections.insert(
                name.clone(),
                Section {
                    name,
                    title: fm.title,
                    sort_by: fm.sort_by,
                    template: fm.template,
                    page_template: fm.page_template,
                    body,
                    content: String::new(),
                    pages: Vec::new(),
                    url,
                },
            );
            continue;
        }
        let url = page_url(&rel, &fm);
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
            earlier: None,
            later: None,
            source: rel,
        });
    }
    site.attach();
    site.build_terms();
    Ok(site)
}

/// Template numbers are integers; a float in front matter would print as
/// something the author did not write, so it is refused up front.
fn check_extra(t: &toml::Table, src: &str) -> Result<()> {
    for (k, v) in t {
        match v {
            toml::Value::Float(_) => return Err(Error::new(src, format!("extra.{k}: floats are not supported, use an integer or a string"))),
            toml::Value::Table(inner) => check_extra(inner, src)?,
            _ => {}
        }
    }
    Ok(())
}

impl Site {
    fn attach(&mut self) {
        for (i, pg) in self.pages.iter().enumerate() {
            if let Some(sec) = self.sections.get_mut(&pg.section) {
                sec.pages.push(i);
            }
        }
        let mut links: Vec<(usize, Option<usize>, Option<usize>)> = Vec::new();
        for sec in self.sections.values_mut() {
            match sec.sort_by.as_str() {
                "weight" => sec.pages.sort_by_key(|&i| self.pages[i].weight),
                "date" => {
                    // Newest first, so the next one along is the older post.
                    sec.pages.sort_by(|&a, &b| self.pages[b].date.cmp(&self.pages[a].date));
                    for (k, &i) in sec.pages.iter().enumerate() {
                        let earlier = sec.pages.get(k + 1).copied();
                        let later = if k > 0 { Some(sec.pages[k - 1]) } else { None };
                        links.push((i, earlier, later));
                    }
                }
                _ => {}
            }
        }
        for (i, earlier, later) in links {
            self.pages[i].earlier = earlier;
            self.pages[i].later = later;
        }
    }

    fn build_terms(&mut self) {
        let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, pg) in self.pages.iter().enumerate() {
            for t in &pg.tags {
                by_name.entry(t.clone()).or_default().push(i);
            }
        }
        for (name, mut pages) in by_name {
            pages.sort_by(|&a, &b| self.pages[b].date.cmp(&self.pages[a].date));
            let url = format!("/tags/{}/", slugify(&name));
            self.terms.push(Term { name, url, pages });
        }
    }

    /// The posts linked to a project by extra.project.
    pub fn posts_for(&self, slug: &str) -> Vec<usize> {
        let Some(sec) = self.sections.get("writing") else { return Vec::new() };
        if slug.is_empty() {
            return Vec::new();
        }
        sec.pages.iter().copied().filter(|&i| extra_str(&self.pages[i].extra, "project") == slug).collect()
    }

    /// A post's project, if it names one that exists.
    pub fn owner_of(&self, page: usize) -> Option<usize> {
        let slug = extra_str(&self.pages[page].extra, "project");
        if slug.is_empty() {
            return None;
        }
        let sec = self.sections.get("projects")?;
        sec.pages.iter().copied().find(|&i| extra_str(&self.pages[i].extra, "slug") == slug)
    }

    /// Zola's `@/path.md` internal link as a URL.
    pub fn resolve(&self, reference: &str) -> Option<String> {
        let rel = reference.strip_prefix("@/").unwrap_or(reference);
        if let Some(dir) = rel.strip_suffix("_index.md") {
            return self.sections.get(dir.trim_end_matches('/')).map(|s| s.url.clone());
        }
        self.pages.iter().find(|p| p.source == rel).map(|p| p.url.clone())
    }
}

pub fn extra_str<'a>(t: &'a toml::Table, key: &str) -> &'a str {
    match t.get(key) {
        Some(toml::Value::String(s)) => s,
        _ => "",
    }
}
