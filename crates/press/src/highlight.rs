//! Syntax highlighting with syntect, class-based: the HTML carries scope
//! classes and syntax.css carries the colours, so the theme is one file the
//! site can override.

use syntect::highlighting::{Theme, ThemeSet};
use syntect::html::{css_for_theme_with_class_style, ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

pub struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

/// Scope names become classes, so they are prefixed: a JSON comma is
/// `punctuation.separator.dictionary.pair`, and an unprefixed `pair` would
/// pick up whatever a theme means by that word.
const STYLE: ClassStyle = ClassStyle::SpacedPrefixed { prefix: "s-" };

impl Highlighter {
    /// `theme` is one of syntect's bundled names; the error for any other
    /// name lists them.
    pub fn new(theme: &str) -> Result<Highlighter, String> {
        let mut themes = ThemeSet::load_defaults();
        let Some(theme) = themes.themes.remove(theme) else {
            let names: Vec<&str> = themes.themes.keys().map(String::as_str).collect();
            return Err(format!("no highlighting theme {theme:?}; the bundled ones are {}", names.join(", ")));
        };
        Ok(Highlighter { syntaxes: SyntaxSet::load_defaults_newlines(), theme })
    }

    /// `<pre class="chroma"><code ...>` with a span per token, or plain
    /// escaped text when the language is unknown.
    pub fn html(&self, src: &str, lang: &str) -> Result<String, String> {
        let syntax = self.syntaxes.find_syntax_by_token(lang).unwrap_or_else(|| self.syntaxes.find_syntax_plain_text());
        let mut gen = ClassedHTMLGenerator::new_with_class_style(syntax, &self.syntaxes, STYLE);
        for line in LinesWithEndings::from(src) {
            gen.parse_html_for_line_which_includes_newline(line).map_err(|e| e.to_string())?;
        }
        let body = gen.finalize();
        Ok(format!("<pre class=\"chroma\"><code>{body}</code></pre>"))
    }

    /// The stylesheet for the classes, without the theme's own background
    /// and text colour: the site paints code slabs itself.
    pub fn css(&self) -> Result<String, String> {
        let css = css_for_theme_with_class_style(&self.theme, STYLE).map_err(|e| e.to_string())?;
        let mut out = String::new();
        let mut skipping = false;
        for line in css.lines() {
            if line.trim_start().starts_with(".code {") {
                skipping = true;
            }
            if !skipping {
                out.push_str(line);
                out.push('\n');
            }
            if skipping && line.trim_end().ends_with('}') {
                skipping = false;
            }
        }
        Ok(out)
    }
}
