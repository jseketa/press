//! Fenced code blocks whose info string names a renderer instead of a
//! language:
//!
//!     ```wave caption="One SWD transaction"
//!     { "signal": [ ... ] }
//!     ```
//!
//! A fence is literal by definition, so there is no escaping decision to get
//! wrong, and it degrades honestly: anything that understands Markdown still
//! shows the source.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};

/// Attributes from a fence info string, `key="value"` and bare flags.
pub type Attrs = BTreeMap<String, String>;

/// Splits an info string into its leading word and attributes:
///
///     dot caption="The two-stage login" class=wide
///
/// Values may be quoted with " or ' and may contain spaces. An unterminated
/// quote takes the rest of the line, which is the forgiving reading.
pub fn parse_info(info: &str) -> (String, Attrs) {
    let b = info.as_bytes();
    let n = b.len();
    let mut i = 0;
    let mut attrs = Attrs::new();
    let skip = |i: &mut usize| {
        while *i < n && (b[*i] == b' ' || b[*i] == b'\t') {
            *i += 1;
        }
    };
    skip(&mut i);
    let start = i;
    while i < n && b[i] != b' ' && b[i] != b'\t' {
        i += 1;
    }
    let name = info[start..i].to_string();
    loop {
        skip(&mut i);
        if i >= n {
            return (name, attrs);
        }
        let key_start = i;
        while i < n && b[i] != b'=' && b[i] != b' ' && b[i] != b'\t' {
            i += 1;
        }
        let key = &info[key_start..i];
        if key.is_empty() {
            return (name, attrs);
        }
        if i >= n || b[i] != b'=' {
            // A bare word is a flag: `wide` means wide="wide".
            attrs.insert(key.to_string(), key.to_string());
            continue;
        }
        i += 1;
        if i < n && (b[i] == b'"' || b[i] == b'\'') {
            let q = b[i];
            i += 1;
            let vs = i;
            while i < n && b[i] != q {
                i += 1;
            }
            attrs.insert(key.to_string(), info[vs..i].to_string());
            if i < n {
                i += 1;
            }
            continue;
        }
        let vs = i;
        while i < n && b[i] != b' ' && b[i] != b'\t' {
            i += 1;
        }
        attrs.insert(key.to_string(), info[vs..i].to_string());
    }
}

fn attr<'a>(a: &'a Attrs, key: &str) -> &'a str {
    a.get(key).map_or("", String::as_str)
}

/// `&`, `<`, `>` and `"`: what attribute values and text content need.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// What a renderer may reach for.
pub struct Env<'a> {
    pub root: &'a Path,
    pub cache_dir: Option<&'a Path>,
    pub highlight: &'a dyn Fn(&str, &str) -> Result<String, String>,
    pub markdown: &'a dyn Fn(&str) -> Result<String, String>,
}

/// One kind of fenced block.
pub struct Block {
    pub name: &'static str,
    /// The client-side library this block needs, if any.
    pub script: Option<&'static str>,
    pub render: fn(&mut String, &str, &Attrs, &Env) -> Result<(), String>,
}

/// Adding a renderer is adding an entry here.
pub const BLOCKS: &[Block] = &[
    Block { name: "mermaid", script: Some("mermaid"), render: mermaid },
    Block { name: "wave", script: Some("wavedrom"), render: wave },
    Block { name: "note", script: None, render: note },
    Block { name: "dot", script: None, render: graphviz },
    Block { name: "bytefield", script: None, render: bytefield },
    Block { name: "pair", script: Some("mermaid"), render: pair },
];

pub fn find(name: &str) -> Option<&'static Block> {
    BLOCKS.iter().find(|b| b.name == name)
}

/// The wrapper every diagram renderer shares: caption and class conventions
/// live in one place.
fn figure(out: &mut String, class: &str, a: &Attrs, body: impl FnOnce(&mut String) -> Result<(), String>) -> Result<(), String> {
    out.push_str(&format!("<figure class=\"{class}\">"));
    body(out)?;
    let caption = attr(a, "caption");
    if !caption.is_empty() {
        out.push_str(&format!("<figcaption>{}</figcaption>", escape(caption)));
    }
    out.push_str("</figure>");
    Ok(())
}

/// Mermaid hands its source to the browser: it needs a DOM to lay out.
fn mermaid(out: &mut String, src: &str, a: &Attrs, _: &Env) -> Result<(), String> {
    figure(out, "diagram", a, |w| {
        // Escaped, not raw: the browser decodes the entities, so textContent
        // hands Mermaid the original `<|--`.
        w.push_str(&format!("<pre class=\"mermaid\">{}</pre>", escape(src)));
        Ok(())
    })
}

/// A WaveDrom timing diagram, client-side.
fn wave(out: &mut String, src: &str, a: &Attrs, _: &Env) -> Result<(), String> {
    figure(out, "diagram diagram--wave", a, |w| {
        // Raw: content inside <script> is raw text to the HTML parser.
        w.push_str(&format!("<script type=\"WaveDrom\">{src}</script>"));
        Ok(())
    })
}

/// A callout whose body is Markdown, rendered through the same pipeline.
fn note(out: &mut String, src: &str, a: &Attrs, env: &Env) -> Result<(), String> {
    let body = (env.markdown)(src)?;
    let kind = a.get("kind").map_or("info", String::as_str);
    let title = a.get("title").map_or("Note", String::as_str);
    out.push_str(&format!("<aside class=\"note note--{}\">", escape(kind)));
    out.push_str(&format!("<p class=\"note__label\">{}</p>", escape(title)));
    out.push_str(&format!("<div class=\"note__body\">{body}</div>"));
    out.push_str("</aside>");
    Ok(())
}

/// Graphviz at build time, inlined so the site's CSS reaches the diagram.
fn graphviz(out: &mut String, src: &str, a: &Attrs, env: &Env) -> Result<(), String> {
    let input = source(env, src, a, "diagrams", ".dot")?;
    let svg = cached(env, "dot", &input, |b| command(&dot_path()?, &["-Tsvg"], Some(b)))?;
    figure(out, "diagram diagram--dot", a, |w| {
        w.push_str(&svg);
        Ok(())
    })
}

/// Prefers whatever is on PATH and falls back to the default Windows install
/// location, which the installer does not add to PATH.
fn dot_path() -> Result<String, String> {
    if Command::new("dot").arg("-V").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok() {
        return Ok("dot".into());
    }
    let fallback = r"C:\Program Files\Graphviz\bin\dot.exe";
    if Path::new(fallback).exists() {
        return Ok(fallback.into());
    }
    Err("graphviz not found: install it, or put dot on PATH".into())
}

/// Bit and byte layouts via the bytefield-svg npm tool; the cache means
/// node's startup is paid once per change, not once per build.
fn bytefield(out: &mut String, src: &str, a: &Attrs, env: &Env) -> Result<(), String> {
    let input = source(env, src, a, "diagrams", ".edn")?;
    let svg = cached(env, "bytefield", &input, |b| {
        // The CLI takes a file, not stdin.
        let tmp = std::env::temp_dir().join(format!("press-{}.edn", std::process::id()));
        std::fs::write(&tmp, b).map_err(|e| e.to_string())?;
        let r = command(&npx(), &["--yes", "bytefield-svg", "-s", &tmp.to_string_lossy()], None);
        let _ = std::fs::remove_file(&tmp);
        r
    })?;
    figure(out, "diagram diagram--bytefield", a, |w| {
        w.push_str(&svg);
        Ok(())
    })
}

fn npx() -> String {
    if cfg!(windows) {
        "npx.cmd".into()
    } else {
        "npx".into()
    }
}

/// The same diagram drawn two ways, for one article comparing the
/// approaches: the fence body is the Mermaid source, src= names the Graphviz
/// half in diagrams/.
fn pair(out: &mut String, src: &str, a: &Attrs, env: &Env) -> Result<(), String> {
    if attr(a, "src").is_empty() {
        return Err("needs src=<name> naming the graphviz half".into());
    }
    let input = source(env, "", a, "diagrams", ".dot")?;
    let svg = cached(env, "dot", &input, |b| command(&dot_path()?, &["-Tsvg"], Some(b)))?;
    figure(out, "diagram diagram--pair", a, |w| {
        w.push_str("<div class=\"pair\">");
        w.push_str("<div class=\"pair__side\"><p class=\"pair__label\">Mermaid<span>in the browser</span></p>");
        w.push_str(&format!("<pre class=\"mermaid\">{}</pre></div>", escape(src)));
        w.push_str("<div class=\"pair__side diagram--dot\"><p class=\"pair__label\">Graphviz<span>at build time</span></p>");
        w.push_str(&svg);
        w.push_str("</div></div>");
        Ok(())
    })
}

/// An ordinary language fence. Attributes promote a plain block to a titled
/// one: ```c file=swd.c
pub fn code(out: &mut String, src: &str, lang: &str, a: &Attrs, env: &Env) -> Result<(), String> {
    let html = (env.highlight)(src, lang)?;
    let (file, label) = (attr(a, "file"), attr(a, "label"));
    if file.is_empty() && label.is_empty() {
        out.push_str(&html);
        return Ok(());
    }
    out.push_str("<figure class=\"code\"><figcaption class=\"code__bar\">");
    out.push_str("<span class=\"code__dots\" aria-hidden=\"true\"></span>");
    if !file.is_empty() {
        out.push_str(&format!("<span class=\"code__file\">{}</span>", escape(file)));
    }
    if !label.is_empty() {
        out.push_str(&format!("<span class=\"code__label\">{}</span>", escape(label)));
    }
    out.push_str("</figcaption>");
    out.push_str(&html);
    out.push_str("</figure>");
    Ok(())
}

/// A renderer's input: the fence body, or the file named by src= for
/// diagrams too long to sit inline.
fn source(env: &Env, body: &str, a: &Attrs, dir: &str, ext: &str) -> Result<String, String> {
    let name = attr(a, "src");
    if name.is_empty() {
        return Ok(body.to_string());
    }
    let p = env.root.join(dir).join(format!("{name}{ext}"));
    std::fs::read_to_string(&p).map_err(|e| format!("src={name:?}: {e}"))
}

/// Runs an external renderer over src, caching the SVG by content hash:
/// external renderers are the slow part of a build and diagram sources
/// change rarely.
fn cached(env: &Env, kind: &str, src: &str, run: impl FnOnce(&str) -> Result<Vec<u8>, String>) -> Result<String, String> {
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    h.update([0u8]);
    h.update(src.as_bytes());
    let key: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let path: Option<PathBuf> = env.cache_dir.map(|d| d.join(format!("{kind}-{key}.svg")));
    if let Some(p) = &path {
        if let Ok(b) = std::fs::read_to_string(p) {
            return Ok(b);
        }
    }
    let out = clean_svg(&String::from_utf8_lossy(&run(src)?));
    if let Some(p) = &path {
        // A failed cache write is not a failed build.
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, &out);
    }
    Ok(out)
}

fn command(name: &str, args: &[&str], stdin: Option<&str>) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new(name);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = cmd.spawn().map_err(|e| format!("{name}: {e}"))?;
    if let Some(input) = stdin {
        let mut pipe = child.stdin.take().unwrap();
        pipe.write_all(input.as_bytes()).map_err(|e| e.to_string())?;
        drop(pipe);
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let msg = if msg.is_empty() { output.status.to_string() } else { msg };
        return Err(format!("{name}: {msg}"));
    }
    Ok(output.stdout)
}

/// Drops the XML prolog and the absolute width/height that stop an SVG
/// scaling with the column; the viewBox carries the aspect ratio.
fn clean_svg(s: &str) -> String {
    let start = s.find("<svg").unwrap_or(0);
    let s = &s[start..];
    let Some(end) = s.find('>') else { return s.trim().to_string() };
    let mut head = String::with_capacity(end);
    let mut rest = &s[..end];
    while let Some(i) = rest.find(" width=\"").or_else(|| rest.find(" height=\"")) {
        // Only bare numbers with an optional pt/px unit go; percentages stay.
        let (before, after) = rest.split_at(i);
        let q1 = after.find('"').unwrap();
        let q2 = after[q1 + 1..].find('"').map(|k| q1 + 1 + k).unwrap_or(after.len() - 1);
        let value = &after[q1 + 1..q2];
        let unitless = value.trim_end_matches("pt").trim_end_matches("px");
        head.push_str(before);
        if !(unitless.chars().all(|c| c.is_ascii_digit() || c == '.') && !unitless.is_empty()) {
            head.push_str(&after[..q2 + 1]);
        }
        rest = &after[q2 + 1..];
    }
    head.push_str(rest);
    format!("{head}{}", &s[end..]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_strings() {
        let (n, a) = parse_info("dot caption=\"The two-stage login\" class=wide flag");
        assert_eq!(n, "dot");
        assert_eq!(a["caption"], "The two-stage login");
        assert_eq!(a["class"], "wide");
        assert_eq!(a["flag"], "flag");
        let (n, a) = parse_info("  go  ");
        assert_eq!(n, "go");
        assert!(a.is_empty());
    }

    #[test]
    fn svg_cleaning() {
        let s = "<?xml version=\"1.0\"?>\n<!DOCTYPE svg>\n<svg width=\"62pt\" height=\"44pt\" viewBox=\"0 0 62 44\" xmlns=\"x\">\n<g/></svg>\n";
        assert_eq!(clean_svg(s), "<svg viewBox=\"0 0 62 44\" xmlns=\"x\">\n<g/></svg>");
        assert_eq!(clean_svg("<svg width=\"100%\"></svg>"), "<svg width=\"100%\"></svg>");
    }
}
