//! Source text, spans and errors.
//!
//! Every AST node carries a span into its source, which gives error
//! locations for free and lets a node reproduce its own text: section 5 of
//! the spec emits literal CSS exactly as written.

use std::fmt;
use std::rc::Rc;

/// One input file: its text, and enough to turn a byte offset back into a
/// line and column when something goes wrong.
pub struct Source {
    pub name: String,
    pub text: String,
    lines: Vec<usize>,
}

impl Source {
    pub fn new(name: &str, text: &str) -> Rc<Source> {
        // Windows editors add a byte order mark and CRLF; the language is
        // defined over LF, as CSS's own preprocessing step is.
        let text = text
            .strip_prefix('\u{feff}')
            .unwrap_or(text)
            .replace("\r\n", "\n")
            .replace(['\r', '\u{c}'], "\n");
        let mut lines = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                lines.push(i + 1);
            }
        }
        Rc::new(Source { name: name.to_string(), text, lines })
    }

    /// The 1-based line and byte column of an offset.
    pub fn pos(&self, off: usize) -> (usize, usize) {
        let off = off.min(self.text.len());
        let line = self.lines.partition_point(|&start| start <= off) - 1;
        (line + 1, off - self.lines[line] + 1)
    }
}

/// A half-open byte range in a source.
#[derive(Clone)]
pub struct Span {
    pub src: Rc<Source>,
    pub a: usize,
    pub b: usize,
}

impl Span {
    pub fn text(&self) -> &str {
        &self.src.text[self.a..self.b]
    }

    /// Text between the end of this span and the start of another.
    pub fn between(&self, other: &Span) -> &str {
        &self.src.text[self.b..other.a]
    }

    pub fn error(&self, msg: impl Into<String>) -> Error {
        let (line, col) = self.src.pos(self.a);
        Error { file: self.src.name.clone(), line, col, msg: msg.into(), frames: Vec::new() }
    }
}

/// A compile error: where, what, and for runtime errors the chain of
/// includes and calls that led there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub msg: String,
    pub frames: Vec<String>,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}: {}", self.file, self.line, self.col, self.msg)?;
        for frame in &self.frames {
            write!(f, "\n  in {frame}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
