//! The acceptance test: building the author's site must produce what the Go
//! press produced, modulo the differences that are by design (whitespace,
//! void-tag spelling, entity spelling, the highlighter's spans compared as
//! text, pulldown's footnote markup, feed timestamps, and two Go bugs: a raw
//! quote in a meta attribute and calendar dates shifted by the machine's
//! UTC offset). Skips when the site is not checked out next to this repo.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn site_dir() -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../jseketa.github.io");
    (p.join("public-press").is_dir() && p.join("config.toml").is_file()).then(|| p.canonicalize().unwrap())
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn replace_all(s: &str, pairs: &[(&str, &str)]) -> String {
    pairs.iter().fold(s.to_string(), |acc, (a, b)| acc.replace(a, b))
}

fn decode(s: &str) -> String {
    replace_all(
        s,
        &[
            ("&#43;", "+"), ("&#39;", "'"), ("&#34;", "\""), ("&quot;", "\""), ("&#160;", " "), ("&nbsp;", " "),
            ("&rsquo;", "\u{2019}"), ("&lsquo;", "\u{2018}"), ("&ldquo;", "\u{201c}"), ("&rdquo;", "\u{201d}"),
            ("&mdash;", "\u{2014}"), ("&ndash;", "\u{2013}"), ("&hellip;", "\u{2026}"),
            ("&lt;", "<"), ("&gt;", ">"), ("&amp;", "&"),
        ],
    )
}

/// Replaces every `<tag ...>...</end>` span with what `f` makes of its body.
fn rewrite_spans(s: &str, open: &str, close: &str, f: impl Fn(&str) -> String) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find(open) {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let Some(end) = after.find(close) else {
            out.push_str(after);
            return out;
        };
        let tag_end = after.find('>').unwrap_or(0) + 1;
        out.push_str(&f(&after[tag_end..end]));
        rest = &after[end + close.len()..];
    }
    out.push_str(rest);
    out
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn normalize_html(s: &str) -> String {
    // Code blocks: text only, compared exactly.
    let s = rewrite_spans(s, "<pre class=\"chroma\">", "</code></pre>", |body| {
        format!("<pre class=\"chroma\"><code>{}</code></pre>", decode(&strip_tags(body)).trim().replace('\n', "<NL>"))
    });
    // The Rust theme dropped subresource integrity: same-origin scripts.
    let mut s = s;
    while let Some(i) = s.find(" integrity=\"") {
        let value = i + " integrity=\"".len();
        let end = s[value..].find('"').map_or(s.len(), |j| value + j + 1);
        s.replace_range(i..end, "");
    }
    // Footnotes: both markups to one shape.
    let s = rewrite_spans(&s, "<sup", "</sup>", |body| format!("<sup>{}</sup>", strip_tags(body).trim()));
    let s = rewrite_spans(&s, "<div class=\"footnotes\"", "</ol>\n</div>", |_| "<FOOTNOTES>".into());
    let s = rewrite_spans(&s, "<div class=\"footnote-definition\"", "</div>", |_| "<FOOTNOTES>".into());
    let s = s.replace("<FOOTNOTES>\n<FOOTNOTES>", "<FOOTNOTES>").replace("<FOOTNOTES><FOOTNOTES>", "<FOOTNOTES>");
    let s = replace_all(
        &s,
        &[
            (" />", ">"), ("/>", ">"), ("&#43;", "+"), ("&#39;", "'"), ("&#34;", "\""), ("&quot;", "\""),
            ("&rsquo;", "\u{2019}"), ("&lsquo;", "\u{2018}"), ("&ldquo;", "\u{201c}"), ("&rdquo;", "\u{201d}"),
            ("&mdash;", "\u{2014}"), ("&ndash;", "\u{2013}"), ("&hellip;", "\u{2026}"),
            ("href=\"/tags\"", "href=\"/tags/\""),
        ],
    );
    // Whitespace.
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !(c == '<' && out.ends_with('>')) {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    out.trim().to_string()
}

fn normalize_feed(s: &str) -> String {
    let mut s = replace_all(
        s,
        &[
            ("&amp;rsquo;", "\u{2019}"), ("&amp;lsquo;", "\u{2018}"), ("&amp;ldquo;", "\u{201c}"), ("&amp;rdquo;", "\u{201d}"),
            ("&amp;mdash;", "\u{2014}"), ("&amp;ndash;", "\u{2013}"), ("&amp;hellip;", "\u{2026}"),
        ],
    );
    // Entry dates: the Go feed wrote 22:00Z of the previous day for a calendar
    // date (local-time parsing); compare by day.
    while let Some(i) = s.find("T22:00:00Z</updated>") {
        let day_start = s[..i].rfind('>').unwrap() + 1;
        let day = &s[day_start..i];
        let (y, m, d) = (day[0..4].parse::<i32>().unwrap(), day[5..7].parse::<u32>().unwrap(), day[8..10].parse::<u32>().unwrap());
        let days_in = [31, if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let (y, m, d) = if d < days_in[m as usize - 1] { (y, m, d + 1) } else if m < 12 { (y, m + 1, 1) } else { (y + 1, 1, 1) };
        s.replace_range(day_start..i + "T22:00:00Z".len(), &format!("{y:04}-{m:02}-{d:02}T00:00:00Z"));
    }
    // The feed's own timestamp is the build time.
    if let (Some(a), Some(b)) = (s.find("<updated>"), s.find("<link rel=\"self\"")) {
        if a < b {
            s.replace_range(a..b, "<updated>NOW</updated>");
        }
    }
    // The escaped HTML in <content> gets the same treatment as a page.
    let s = rewrite_spans(&s, "<content type=\"html\">", "</content>", |body| {
        let html = replace_all(body, &[("&lt;", "<"), ("&gt;", ">"), ("&#34;", "\""), ("&#39;", "'"), ("&amp;", "&")]);
        format!("<content type=\"html\">{}</content>", normalize_html(&html))
    });
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn site_matches_go_press() {
    let Some(site) = site_dir() else {
        eprintln!("site not checked out next to this repo; skipping");
        return;
    };
    let out = std::env::temp_dir().join(format!("press-rs-site-test-{}", std::process::id()));
    let status = Command::new(env!("CARGO_BIN_EXE_press"))
        .args(["-site", &site.to_string_lossy(), "-out", &out.to_string_lossy()])
        .args(["-templates", &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../theme").to_string_lossy()])
        .args(["-cache", &site.join(".press-cache").to_string_lossy()])
        .status()
        .unwrap();
    assert!(status.success(), "press failed");

    let want_dir = site.join("public-press");
    let mut files = Vec::new();
    walk(&want_dir, &mut files);
    let mut failures = Vec::new();
    for want_path in files {
        let rel = want_path.strip_prefix(&want_dir).unwrap();
        let name = rel.to_string_lossy().replace('\\', "/");
        if name == "syntax.css" {
            continue; // a different highlighter, by design
        }
        let got_path = out.join(rel);
        let (Ok(want), Ok(got)) = (fs::read(&want_path), fs::read(&got_path)) else {
            failures.push(format!("{name}: missing in output"));
            continue;
        };
        let same = if name.ends_with(".html") {
            normalize_html(&String::from_utf8_lossy(&want)) == normalize_html(&String::from_utf8_lossy(&got))
        } else if name == "atom.xml" {
            normalize_feed(&String::from_utf8_lossy(&want)) == normalize_feed(&String::from_utf8_lossy(&got))
        } else if name == "main.css" {
            let n = |b: &[u8]| String::from_utf8_lossy(b).split_whitespace().collect::<String>();
            n(&want) == n(&got)
        } else {
            want == got
        };
        if !same {
            failures.push(format!("{name}: differs"));
        }
    }
    let _ = fs::remove_dir_all(&out);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
