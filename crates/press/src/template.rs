//! The template language: HTML plus insert, repeat, choose and compose.
//! docs/templates.md is the definition. The engine knows nothing about
//! sites: pages, sections and terms arrive as objects behind a trait, and
//! `url()` is answered by a host.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::rc::Rc;

use crate::error::{Error, Result};

// --- values ---------------------------------------------------------------

/// A calendar day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
];

impl Date {
    /// `%Y %m %d %b %B`; anything else is copied.
    pub fn format(&self, fmt: &str) -> String {
        let mut out = String::new();
        let mut chars = fmt.chars();
        let month = MONTHS[(self.month as usize).clamp(1, 12) - 1];
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('Y') => out.push_str(&self.year.to_string()),
                Some('m') => out.push_str(&format!("{:02}", self.month)),
                Some('d') => out.push_str(&format!("{:02}", self.day)),
                Some('b') => out.push_str(&month[..3]),
                Some('B') => out.push_str(month),
                Some(o) => {
                    out.push('%');
                    out.push(o);
                }
                None => out.push('%'),
            }
        }
        out
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// An object with a fixed set of fields: a page, a section, a term, a
/// year group, config, site. Reading a field it does not have is an error.
pub trait Object {
    fn kind(&self) -> &'static str;
    fn field(&self, name: &str) -> Option<Value>;
}

#[derive(Clone)]
pub enum Value {
    Null,
    Bool(bool),
    Num(i64),
    Str(Rc<str>),
    Html(Rc<str>),
    Date(Date),
    List(Rc<Vec<Value>>),
    /// `extra` and its tables: a missing key is null.
    Map(Rc<BTreeMap<String, Value>>),
    /// `site.sections`: a missing key is an error.
    StrictMap(Rc<BTreeMap<String, Value>>),
    Object(Rc<dyn Object>),
}

impl Value {
    pub fn str(s: impl Into<Rc<str>>) -> Value {
        Value::Str(s.into())
    }

    pub fn html(s: impl Into<Rc<str>>) -> Value {
        Value::Html(s.into())
    }

    pub fn list(items: Vec<Value>) -> Value {
        Value::List(Rc::new(items))
    }

    pub fn object(o: impl Object + 'static) -> Value {
        Value::Object(Rc::new(o))
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Num(_) => "number",
            Value::Str(_) => "string",
            Value::Html(_) => "html",
            Value::Date(_) => "date",
            Value::List(_) => "list",
            Value::Map(_) | Value::StrictMap(_) => "map",
            Value::Object(o) => o.kind(),
        }
    }

    /// Null, false, 0, an empty string or empty html, an empty list.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null | Value::Bool(false) | Value::Num(0) => false,
            Value::Str(s) | Value::Html(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
            _ => true,
        }
    }
}

fn equal(a: &Value, b: &Value) -> std::result::Result<bool, String> {
    Ok(match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Num(x), Value::Num(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Html(x), Value::Html(y)) => x == y,
        (Value::Date(x), Value::Date(y)) => x == y,
        _ => return Err(format!("cannot compare {} with {}", a.kind(), b.kind())),
    })
}

/// The text `{{ }}` inserts, before escaping; None for values with no text.
fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::Str(s) | Value::Html(s) => Some(s.to_string()),
        Value::Num(n) => Some(n.to_string()),
        Value::Date(d) => Some(d.to_string()),
        _ => None,
    }
}

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// What the site provides to templates beyond data: URLs.
pub trait Host {
    fn url(&self, path: &str) -> std::result::Result<String, String>;
}

/// The names every template may read without being given them.
pub const GLOBALS: &[&str] = &["config", "site", "scripts", "current_path", "year", "page", "section", "term"];

const KEYWORDS: &[&str] = &["if", "else", "end", "for", "in", "let", "include", "extend", "yield", "and", "or", "not", "true", "false"];

// --- syntax ---------------------------------------------------------------

#[derive(Clone, Copy)]
struct Pos(usize);

enum Expr {
    Str(String, Pos),
    Num(i64, Pos),
    Bool(bool, Pos),
    Var(String, Pos),
    Field(Box<Expr>, String, Pos),
    Call(String, Vec<Expr>, Pos),
    Bin(BinOp, Box<Expr>, Box<Expr>, Pos),
    Not(Box<Expr>, Pos),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Add,
}

impl Expr {
    fn pos(&self) -> Pos {
        match self {
            Expr::Str(_, p) | Expr::Num(_, p) | Expr::Bool(_, p) | Expr::Var(_, p) | Expr::Field(_, _, p) | Expr::Call(_, _, p) | Expr::Bin(_, _, _, p) | Expr::Not(_, p) => *p,
        }
    }
}

enum Node {
    Text(String),
    Insert(Expr),
    If(Vec<(Option<Expr>, Vec<Node>)>),
    For { index: Option<String>, var: String, list: Expr, body: Vec<Node> },
    Let { name: String, value: Expr, pos: Pos },
    Include { file: String, args: Vec<(String, Expr)>, pos: Pos },
    Yield(Pos),
}

struct Extend {
    file: String,
    args: Vec<(String, Expr)>,
    pos: Pos,
}

/// One parsed theme file.
pub struct Template {
    name: String,
    lines: Vec<usize>,
    extend: Option<Extend>,
    body: Vec<Node>,
    yields: Vec<Pos>,
    /// Names read without being bound by a for or let in the file.
    free: HashSet<String>,
}

fn line_col(lines: &[usize], off: usize) -> (usize, usize) {
    let line = lines.partition_point(|&s| s <= off).max(1) - 1;
    (line + 1, off - lines[line] + 1)
}

impl Template {
    fn error(&self, at: Pos, msg: impl Into<String>) -> Error {
        let (line, col) = line_col(&self.lines, at.0);
        Error::at(self.name.clone(), line, col, msg)
    }

    fn is_partial(&self) -> bool {
        short_name(&self.name).starts_with('_')
    }
}

fn short_name(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

// --- lexing and the standalone-line rule -------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum TagKind {
    Insert,
    Stmt,
    Comment,
}

enum Piece {
    Text(usize, usize),
    Newline,
    Tag { kind: TagKind, inner: (usize, usize), at: usize },
}

/// Splits a file into text, newlines and tags, then removes standalone
/// lines: a line whose text apart from `{% %}` and `{# #}` tags is only
/// spaces and tabs goes away whole, line ending included.
fn lex(name: &str, src: &str, lines: &[usize]) -> Result<Vec<Piece>> {
    let b = src.as_bytes();
    let mut pieces = Vec::new();
    let mut i = 0;
    let mut text_start = 0;
    let flush_text = |pieces: &mut Vec<Piece>, from: usize, to: usize| {
        let mut s = from;
        for (k, &c) in b[from..to].iter().enumerate() {
            if c == b'\n' {
                if from + k > s {
                    pieces.push(Piece::Text(s, from + k));
                }
                pieces.push(Piece::Newline);
                s = from + k + 1;
            }
        }
        if to > s {
            pieces.push(Piece::Text(s, to));
        }
    };
    while i < b.len() {
        if b[i] == b'{' && i + 1 < b.len() && matches!(b[i + 1], b'{' | b'%' | b'#') {
            flush_text(&mut pieces, text_start, i);
            let (kind, closer): (TagKind, &[u8]) = match b[i + 1] {
                b'{' => (TagKind::Insert, b"}}"),
                b'%' => (TagKind::Stmt, b"%}"),
                _ => (TagKind::Comment, b"#}"),
            };
            let mut j = i + 2;
            let end = loop {
                if j >= b.len() {
                    let (line, col) = line_col(lines, i);
                    return Err(Error::at(name, line, col, "unclosed tag"));
                }
                // The closer counts only between tokens: not inside a string.
                if kind != TagKind::Comment && b[j] == b'"' {
                    j += 1;
                    while j < b.len() && b[j] != b'"' && b[j] != b'\n' {
                        if b[j] == b'\\' {
                            j += 1;
                        }
                        j += 1;
                    }
                    j += 1;
                    continue;
                }
                if b[j..].starts_with(closer) {
                    break j;
                }
                j += 1;
            };
            pieces.push(Piece::Tag { kind, inner: (i + 2, end), at: i });
            i = end + 2;
            text_start = i;
            continue;
        }
        i += 1;
    }
    flush_text(&mut pieces, text_start, b.len());

    // The standalone-line rule, line by line.
    let mut out: Vec<Piece> = Vec::with_capacity(pieces.len());
    let mut line: Vec<Piece> = Vec::new();
    let finish = |line: &mut Vec<Piece>, out: &mut Vec<Piece>, newline: bool| {
        let has_stmt = line.iter().any(|p| matches!(p, Piece::Tag { kind: TagKind::Stmt | TagKind::Comment, .. }));
        let clean = line.iter().all(|p| match p {
            Piece::Text(a, z) => src[*a..*z].bytes().all(|c| c == b' ' || c == b'\t'),
            Piece::Tag { kind, .. } => *kind != TagKind::Insert,
            Piece::Newline => true,
        });
        if has_stmt && clean {
            out.extend(line.drain(..).filter(|p| matches!(p, Piece::Tag { .. })));
        } else {
            out.append(line);
            if newline {
                out.push(Piece::Newline);
            }
        }
    };
    for p in pieces {
        match p {
            Piece::Newline => finish(&mut line, &mut out, true),
            other => line.push(other),
        }
    }
    finish(&mut line, &mut out, false);
    Ok(out)
}

// --- tokens ---------------------------------------------------------------

enum Tok {
    Name(String, usize),
    Num(i64, usize),
    Str(String, usize),
    Punct(&'static str, usize),
}

impl Tok {
    fn pos(&self) -> usize {
        match self {
            Tok::Name(_, p) | Tok::Num(_, p) | Tok::Str(_, p) | Tok::Punct(_, p) => *p,
        }
    }
}

/// Tokens of one tag body. Errors carry the absolute offset in `col`; the
/// parser turns them into located errors.
struct Lexer<'a> {
    s: &'a str,
    off: usize,
    i: usize,
}

impl<'a> Lexer<'a> {
    fn new(s: &'a str, off: usize) -> Lexer<'a> {
        Lexer { s, off, i: 0 }
    }

    fn next_token(&mut self) -> std::result::Result<Option<Tok>, (usize, String)> {
        let b = self.s.as_bytes();
        while self.i < b.len() && b[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
        if self.i >= b.len() {
            return Ok(None);
        }
        let at = self.off + self.i;
        let c = b[self.i];
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = self.i;
            while self.i < b.len() && (b[self.i].is_ascii_alphanumeric() || b[self.i] == b'_') {
                self.i += 1;
            }
            return Ok(Some(Tok::Name(self.s[start..self.i].to_string(), at)));
        }
        if c.is_ascii_digit() {
            let start = self.i;
            while self.i < b.len() && b[self.i].is_ascii_digit() {
                self.i += 1;
            }
            let n: i64 = self.s[start..self.i].parse().map_err(|_| (at, "number too large".to_string()))?;
            return Ok(Some(Tok::Num(n, at)));
        }
        if c == b'"' {
            self.i += 1;
            let mut out = String::new();
            loop {
                let Some(ch) = self.s[self.i..].chars().next() else {
                    return Err((at, "unterminated string".into()));
                };
                match ch {
                    '"' => {
                        self.i += 1;
                        break;
                    }
                    '\n' => return Err((at, "strings are one line".into())),
                    // Only `\"` and `\\` are escapes; any other backslash is itself.
                    '\\' if matches!(b.get(self.i + 1), Some(b'"' | b'\\')) => {
                        out.push(b[self.i + 1] as char);
                        self.i += 2;
                    }
                    ch => {
                        out.push(ch);
                        self.i += ch.len_utf8();
                    }
                }
            }
            return Ok(Some(Tok::Str(out, at)));
        }
        for p in ["==", "!=", ".", "(", ")", ",", "+", "="] {
            if self.s[self.i..].starts_with(p) {
                self.i += p.len();
                return Ok(Some(Tok::Punct(p, at)));
            }
        }
        let ch = self.s[self.i..].chars().next().unwrap();
        Err((at, format!("unexpected {ch:?}")))
    }
}

// --- parsing --------------------------------------------------------------

struct Parser<'a> {
    name: &'a str,
    src: &'a str,
    lines: &'a [usize],
    pieces: Vec<Piece>,
    i: usize,
    /// Names bound by enclosing for/let, innermost last.
    bound: Vec<HashSet<String>>,
    free: HashSet<String>,
    yields: Vec<Pos>,
}

/// What a body ended with: `end` or `else` with the tag's text and offsets.
struct Stop {
    keyword: String,
    at: usize,
    rest: (usize, usize),
}

impl<'a> Parser<'a> {
    fn error(&self, at: usize, msg: impl Into<String>) -> Error {
        let (line, col) = line_col(self.lines, at);
        Error::at(self.name, line, col, msg)
    }

    /// The absolute offset of a subslice of the source.
    fn off(&self, sub: &str) -> usize {
        sub.as_ptr() as usize - self.src.as_ptr() as usize
    }

    fn tok(&self, lx: &mut Lexer<'_>) -> Result<Option<Tok>> {
        lx.next_token().map_err(|(at, msg)| self.error(at, msg))
    }

    fn is_bound(&self, name: &str) -> bool {
        self.bound.iter().any(|s| s.contains(name))
    }

    /// Statements up to an `end`/`else` tag (returned) or the end of the file.
    fn nodes(&mut self, in_body: bool) -> Result<(Vec<Node>, Option<Stop>)> {
        let mut out = Vec::new();
        while self.i < self.pieces.len() {
            let p = &self.pieces[self.i];
            self.i += 1;
            match *p {
                Piece::Text(a, z) => out.push(Node::Text(self.src[a..z].to_string())),
                Piece::Newline => out.push(Node::Text("\n".into())),
                Piece::Tag { kind: TagKind::Comment, .. } => {}
                Piece::Tag { kind: TagKind::Insert, inner, .. } => {
                    let e = self.expr_at(inner)?;
                    out.push(Node::Insert(e));
                }
                Piece::Tag { kind: TagKind::Stmt, inner, at } => {
                    let text = &self.src[inner.0..inner.1];
                    let head = text.trim_start();
                    let kw_len = head.find(|c: char| c.is_whitespace()).unwrap_or(head.len());
                    let kw = &head[..kw_len];
                    let rest = head[kw_len..].trim();
                    let rest = (self.off(rest), self.off(rest) + rest.len());
                    let rest_text = &self.src[rest.0..rest.1];
                    match kw {
                        "end" | "else" => {
                            if !in_body {
                                return Err(self.error(at, format!("unexpected {kw}")));
                            }
                            if kw == "end" && !rest_text.is_empty() {
                                return Err(self.error(rest.0, "end takes no keyword"));
                            }
                            return Ok((out, Some(Stop { keyword: kw.to_string(), at, rest })));
                        }
                        "if" => {
                            let mut branches = Vec::new();
                            let mut cond = Some(self.expr_at(rest)?);
                            loop {
                                let (body, stop) = self.nodes(true)?;
                                branches.push((cond.take(), body));
                                let stop = stop.ok_or_else(|| self.error(at, "unclosed if"))?;
                                if stop.keyword == "end" {
                                    break;
                                }
                                let else_text = &self.src[stop.rest.0..stop.rest.1];
                                if else_text.is_empty() {
                                    let (body, stop) = self.nodes(true)?;
                                    branches.push((None, body));
                                    match stop {
                                        Some(s) if s.keyword == "end" => break,
                                        Some(s) => return Err(self.error(s.at, "only one else per if")),
                                        None => return Err(self.error(at, "unclosed if")),
                                    }
                                }
                                let mut lx = Lexer::new(else_text, stop.rest.0);
                                match self.tok(&mut lx)? {
                                    Some(Tok::Name(n, _)) if n == "if" => {}
                                    _ => return Err(self.error(stop.rest.0, "expected else or else if")),
                                }
                                let (c, next) = self.parse_expr_tokens(&mut lx)?;
                                if let Some(t) = next {
                                    return Err(self.error(t.pos(), "unexpected text after expression"));
                                }
                                cond = Some(c);
                            }
                            out.push(Node::If(branches));
                        }
                        "for" => {
                            let mut lx = Lexer::new(rest_text, rest.0);
                            let name = |t: Option<Tok>| -> Result<(String, usize)> {
                                match t {
                                    Some(Tok::Name(n, p)) => Ok((n, p)),
                                    Some(t) => Err(self.error(t.pos(), "expected a name")),
                                    None => Err(self.error(at, "expected: for x in list")),
                                }
                            };
                            let (first, first_at) = name(self.tok(&mut lx)?)?;
                            let (index, var) = match self.tok(&mut lx)? {
                                Some(Tok::Punct(",", _)) => {
                                    let (v, v_at) = name(self.tok(&mut lx)?)?;
                                    match self.tok(&mut lx)? {
                                        Some(Tok::Name(n, _)) if n == "in" => {}
                                        _ => return Err(self.error(at, "expected: for i, x in list")),
                                    }
                                    if v == first {
                                        return Err(self.error(v_at, "index and item have the same name"));
                                    }
                                    (Some((first, first_at)), (v, v_at))
                                }
                                Some(Tok::Name(n, _)) if n == "in" => (None, (first, first_at)),
                                _ => return Err(self.error(at, "expected: for x in list")),
                            };
                            for (n, p) in index.iter().chain(std::iter::once(&var)) {
                                self.check_name(n, *p)?;
                            }
                            let (list, next) = self.parse_expr_tokens(&mut lx)?;
                            if let Some(t) = next {
                                return Err(self.error(t.pos(), "unexpected text after expression"));
                            }
                            let mut names: HashSet<String> = HashSet::new();
                            names.insert(var.0.clone());
                            if let Some((i, _)) = &index {
                                names.insert(i.clone());
                            }
                            self.bound.push(names);
                            let parsed = self.nodes(true);
                            self.bound.pop();
                            let (body_nodes, stop) = parsed?;
                            match stop {
                                Some(s) if s.keyword == "end" => {}
                                Some(s) => return Err(self.error(s.at, "else is not allowed in for")),
                                None => return Err(self.error(at, "unclosed for")),
                            }
                            out.push(Node::For { index: index.map(|(n, _)| n), var: var.0, list, body: body_nodes });
                        }
                        "let" => {
                            let mut lx = Lexer::new(rest_text, rest.0);
                            let (name, name_at) = match self.tok(&mut lx)? {
                                Some(Tok::Name(n, p)) => (n, p),
                                _ => return Err(self.error(at, "expected: let name = value")),
                            };
                            self.check_name(&name, name_at)?;
                            match self.tok(&mut lx)? {
                                Some(Tok::Punct("=", _)) => {}
                                _ => return Err(self.error(at, "expected: let name = value")),
                            }
                            let (value, next) = self.parse_expr_tokens(&mut lx)?;
                            if let Some(t) = next {
                                return Err(self.error(t.pos(), "unexpected text after expression"));
                            }
                            if let Some(scope) = self.bound.last_mut() {
                                scope.insert(name.clone());
                            }
                            out.push(Node::Let { name, value, pos: Pos(name_at) });
                        }
                        "include" => {
                            let (file, args) = self.file_and_args(rest, at)?;
                            if !file.starts_with('_') {
                                return Err(self.error(at, format!("include names a partial (_*.html), not {file:?}")));
                            }
                            out.push(Node::Include { file, args, pos: Pos(at) });
                        }
                        "extend" => return Err(self.error(at, "extend must be the first statement of a page template")),
                        "yield" => {
                            if !rest_text.is_empty() {
                                return Err(self.error(rest.0, "yield takes no arguments"));
                            }
                            self.yields.push(Pos(at));
                            out.push(Node::Yield(Pos(at)));
                        }
                        other => return Err(self.error(at, format!("unknown statement {other:?}"))),
                    }
                }
            }
        }
        Ok((out, None))
    }

    fn check_name(&self, name: &str, at: usize) -> Result<()> {
        if KEYWORDS.contains(&name) {
            return Err(self.error(at, format!("{name} is a keyword")));
        }
        Ok(())
    }

    /// `"file.html" name = expr, name = expr` for include and extend.
    fn file_and_args(&mut self, rest: (usize, usize), at: usize) -> Result<(String, Vec<(String, Expr)>)> {
        let mut lx = Lexer::new(&self.src[rest.0..rest.1], rest.0);
        let file = match self.tok(&mut lx)? {
            Some(Tok::Str(s, _)) => s,
            _ => return Err(self.error(at, "expected a quoted file name")),
        };
        let mut args = Vec::new();
        let mut seen = HashSet::new();
        let mut next = self.tok(&mut lx)?;
        loop {
            match next {
                None => break,
                Some(Tok::Name(name, p)) => {
                    if !seen.insert(name.clone()) {
                        return Err(self.error(p, format!("argument {name} given twice")));
                    }
                    self.check_name(&name, p)?;
                    match self.tok(&mut lx)? {
                        Some(Tok::Punct("=", _)) => {}
                        _ => return Err(self.error(p, format!("expected = after {name}"))),
                    }
                    let (e, after) = self.parse_expr_tokens(&mut lx)?;
                    args.push((name, e));
                    match after {
                        None => break,
                        Some(Tok::Punct(",", p)) => {
                            next = self.tok(&mut lx)?;
                            if next.is_none() {
                                return Err(self.error(p, "expected an argument after ,"));
                            }
                        }
                        Some(t) => return Err(self.error(t.pos(), "expected , between arguments")),
                    }
                }
                Some(t) => return Err(self.error(t.pos(), "expected an argument name")),
            }
        }
        Ok((file, args))
    }

    fn expr_at(&mut self, span: (usize, usize)) -> Result<Expr> {
        let mut lx = Lexer::new(&self.src[span.0..span.1], span.0);
        let (e, next) = self.parse_expr_tokens(&mut lx)?;
        if let Some(t) = next {
            return Err(self.error(t.pos(), "unexpected text after expression"));
        }
        Ok(e)
    }

    /// One expression and the token that follows it.
    fn parse_expr_tokens(&mut self, lx: &mut Lexer<'_>) -> Result<(Expr, Option<Tok>)> {
        let first = self.tok(lx)?;
        let mut ep = ExprParser { p: self, lx, cur: first };
        let e = ep.or()?;
        let next = ep.cur.take();
        Ok((e, next))
    }
}

struct ExprParser<'a, 'b, 'c> {
    p: &'b mut Parser<'a>,
    lx: &'b mut Lexer<'c>,
    cur: Option<Tok>,
}

impl<'a, 'b, 'c> ExprParser<'a, 'b, 'c> {
    fn advance(&mut self) -> Result<()> {
        self.cur = self.p.tok(self.lx)?;
        Ok(())
    }

    fn is_name(&self, kw: &str) -> bool {
        matches!(&self.cur, Some(Tok::Name(n, _)) if n == kw)
    }

    fn is_punct(&self, p: &str) -> bool {
        matches!(&self.cur, Some(Tok::Punct(q, _)) if *q == p)
    }

    fn here(&self) -> usize {
        self.cur.as_ref().map_or(self.lx.off + self.lx.i, Tok::pos)
    }

    fn or(&mut self) -> Result<Expr> {
        let mut l = self.and()?;
        while self.is_name("or") {
            let at = self.here();
            self.advance()?;
            let r = self.and()?;
            l = Expr::Bin(BinOp::Or, Box::new(l), Box::new(r), Pos(at));
        }
        Ok(l)
    }

    fn and(&mut self) -> Result<Expr> {
        let mut l = self.not()?;
        while self.is_name("and") {
            let at = self.here();
            self.advance()?;
            let r = self.not()?;
            l = Expr::Bin(BinOp::And, Box::new(l), Box::new(r), Pos(at));
        }
        Ok(l)
    }

    fn not(&mut self) -> Result<Expr> {
        if self.is_name("not") {
            let at = self.here();
            self.advance()?;
            let x = self.not()?;
            return Ok(Expr::Not(Box::new(x), Pos(at)));
        }
        self.eq()
    }

    fn eq(&mut self) -> Result<Expr> {
        let mut l = self.add()?;
        loop {
            let op = if self.is_punct("==") {
                BinOp::Eq
            } else if self.is_punct("!=") {
                BinOp::Ne
            } else {
                return Ok(l);
            };
            let at = self.here();
            self.advance()?;
            let r = self.add()?;
            l = Expr::Bin(op, Box::new(l), Box::new(r), Pos(at));
        }
    }

    fn add(&mut self) -> Result<Expr> {
        let mut l = self.postfix()?;
        while self.is_punct("+") {
            let at = self.here();
            self.advance()?;
            let r = self.postfix()?;
            l = Expr::Bin(BinOp::Add, Box::new(l), Box::new(r), Pos(at));
        }
        Ok(l)
    }

    fn postfix(&mut self) -> Result<Expr> {
        let mut e = self.primary()?;
        while self.is_punct(".") {
            self.advance()?;
            match self.cur.take() {
                Some(Tok::Name(name, at)) => {
                    self.advance()?;
                    e = Expr::Field(Box::new(e), name, Pos(at));
                }
                other => {
                    let at = other.as_ref().map_or(self.lx.off + self.lx.i, Tok::pos);
                    return Err(self.p.error(at, "expected a field name after ."));
                }
            }
        }
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr> {
        let tok = self.cur.take();
        match tok {
            Some(Tok::Str(s, at)) => {
                self.advance()?;
                Ok(Expr::Str(s, Pos(at)))
            }
            Some(Tok::Num(n, at)) => {
                self.advance()?;
                Ok(Expr::Num(n, Pos(at)))
            }
            Some(Tok::Punct("(", at)) => {
                self.advance()?;
                let e = self.or()?;
                if !self.is_punct(")") {
                    return Err(self.p.error(at, "unclosed ("));
                }
                self.advance()?;
                Ok(e)
            }
            Some(Tok::Name(name, at)) => {
                self.advance()?;
                match name.as_str() {
                    "true" => return Ok(Expr::Bool(true, Pos(at))),
                    "false" => return Ok(Expr::Bool(false, Pos(at))),
                    kw if KEYWORDS.contains(&kw) => return Err(self.p.error(at, format!("unexpected {kw}"))),
                    _ => {}
                }
                if self.is_punct("(") {
                    self.advance()?;
                    let mut args = Vec::new();
                    if !self.is_punct(")") {
                        loop {
                            args.push(self.or()?);
                            if self.is_punct(",") {
                                self.advance()?;
                                continue;
                            }
                            break;
                        }
                    }
                    if !self.is_punct(")") {
                        return Err(self.p.error(at, format!("unclosed call of {name}")));
                    }
                    self.advance()?;
                    return Ok(Expr::Call(name, args, Pos(at)));
                }
                if !self.p.is_bound(&name) {
                    self.p.free.insert(name.clone());
                }
                Ok(Expr::Var(name, Pos(at)))
            }
            Some(t) => Err(self.p.error(t.pos(), "expected a value")),
            None => Err(self.p.error(self.lx.off + self.lx.i, "expected a value")),
        }
    }
}

fn parse_template(name: &str, src: &str) -> Result<Template> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src).replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = vec![0];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            lines.push(i + 1);
        }
    }
    let pieces = lex(name, &src, &lines)?;
    let mut p = Parser { name, src: &src, lines: &lines, pieces, i: 0, bound: vec![HashSet::new()], free: HashSet::new(), yields: Vec::new() };
    // extend: the first statement, after whitespace and comments.
    let mut extend = None;
    let mut k = 0;
    while k < p.pieces.len() {
        match p.pieces[k] {
            Piece::Newline => k += 1,
            Piece::Text(a, z) if src[a..z].trim().is_empty() => k += 1,
            Piece::Tag { kind: TagKind::Comment, .. } => k += 1,
            Piece::Tag { kind: TagKind::Stmt, inner, at } => {
                let text = src[inner.0..inner.1].trim_start();
                if let Some(rest) = text.strip_prefix("extend").filter(|r| r.starts_with(|c: char| c.is_whitespace())) {
                    let rest = rest.trim();
                    let span = (p.off(rest), p.off(rest) + rest.len());
                    let (file, args) = p.file_and_args(span, at)?;
                    extend = Some(Extend { file, args, pos: Pos(at) });
                    p.i = k + 1;
                }
                break;
            }
            _ => break,
        }
    }
    let (body, stop) = p.nodes(false)?;
    if let Some(s) = stop {
        return Err(p.error(s.at, format!("unexpected {}", s.keyword)));
    }
    let (free, yields) = (p.free, p.yields);
    Ok(Template { name: name.to_string(), lines, extend, body, yields, free })
}

// --- the theme and rendering ------------------------------------------------

/// The variables a render starts with: the globals plus the page-kind value.
pub type Vars = HashMap<String, Value>;

struct Scope {
    vars: HashMap<String, Value>,
}

struct Render<'a> {
    theme: &'a Theme,
    host: &'a dyn Host,
    globals: &'a HashMap<String, Value>,
}

/// A theme: every `theme/*.html` parsed once and checked as a whole.
pub struct Theme {
    templates: BTreeMap<String, Rc<Template>>,
}

impl Theme {
    /// Parses every file; `files` maps `theme/name.html` to its text.
    pub fn load(files: &[(String, String)]) -> Result<Theme> {
        let mut templates = BTreeMap::new();
        for (name, src) in files {
            templates.insert(short_name(name).to_string(), Rc::new(parse_template(name, src)?));
        }
        let theme = Theme { templates };
        theme.check()?;
        Ok(theme)
    }

    /// The static rules, in an order that reports the root cause: every
    /// extend (target exists, is not a partial, does not extend, has one
    /// yield, is given what it reads), then stray yields, then includes
    /// (exist, are partials, arguments match the free names, no cycles).
    fn check(&self) -> Result<()> {
        let mut bases: HashSet<&str> = HashSet::new();
        for t in self.templates.values() {
            let Some(e) = &t.extend else { continue };
            if t.is_partial() {
                return Err(t.error(e.pos, "a partial cannot extend"));
            }
            let Some(base) = self.templates.get(&e.file) else {
                return Err(t.error(e.pos, format!("no template {:?} to extend", e.file)));
            };
            if base.is_partial() {
                return Err(t.error(e.pos, format!("{:?} is a partial; extend a base template", e.file)));
            }
            if let Some(be) = &base.extend {
                return Err(base.error(be.pos, format!("{} is extended by {}, so it cannot extend", short_name(&base.name), short_name(&t.name))));
            }
            match base.yields.len() {
                1 => {}
                0 => return Err(t.error(e.pos, format!("{:?} has no yield", e.file))),
                _ => return Err(base.error(base.yields[1], "a base has exactly one yield")),
            }
            self.check_args(t, e.pos, &e.file, base, &e.args)?;
            bases.insert(e.file.as_str());
        }
        for (name, t) in &self.templates {
            if let Some(&at) = t.yields.first() {
                if !bases.contains(name.as_str()) {
                    return Err(t.error(at, "yield is only allowed in a template that another extends"));
                }
            }
            self.check_includes(t, &t.body, &mut vec![name.clone()])?;
        }
        Ok(())
    }

    /// The arguments an include or extend passes must be exactly the names
    /// the target reads beyond the globals.
    fn check_args(&self, from: &Template, at: Pos, file: &str, target: &Template, args: &[(String, Expr)]) -> Result<()> {
        for (name, _) in args {
            if !target.free.contains(name) {
                return Err(from.error(at, format!("{file:?} does not read {name}")));
            }
        }
        for name in &target.free {
            if !GLOBALS.contains(&name.as_str()) && !args.iter().any(|(a, _)| a == name) {
                return Err(from.error(at, format!("{file:?} reads {name}, which is not given")));
            }
        }
        Ok(())
    }

    fn check_includes(&self, t: &Template, nodes: &[Node], stack: &mut Vec<String>) -> Result<()> {
        for n in nodes {
            match n {
                Node::Include { file, args, pos } => {
                    let Some(partial) = self.templates.get(file) else {
                        return Err(t.error(*pos, format!("no partial {file:?}")));
                    };
                    self.check_args(t, *pos, file, partial, args)?;
                    if stack.contains(file) {
                        return Err(t.error(*pos, format!("include cycle through {file:?}")));
                    }
                    stack.push(file.clone());
                    self.check_includes(partial, &partial.body, stack)?;
                    stack.pop();
                }
                Node::If(branches) => {
                    for (_, body) in branches {
                        self.check_includes(t, body, stack)?;
                    }
                }
                Node::For { body, .. } => self.check_includes(t, body, stack)?,
                _ => {}
            }
        }
        Ok(())
    }

    /// Renders a page template with the globals and its page-kind variable.
    /// The arguments of `extend` are evaluated after the body, so they may
    /// use names the body `let`s.
    pub fn render(&self, name: &str, globals: &Vars, host: &dyn Host) -> Result<String> {
        let t = self.templates.get(name).filter(|_| !name.starts_with('_')).ok_or_else(|| Error::new(format!("theme/{name}"), "no such page template"))?;
        let mut r = Render { theme: self, host, globals };
        let mut scope = Scope { vars: HashMap::new() };
        let mut body = String::new();
        r.nodes(t, &t.body, &mut scope, &mut body, None)?;
        let Some(e) = &t.extend else { return Ok(body) };
        let base = &self.templates[&e.file];
        let mut base_scope = Scope { vars: HashMap::new() };
        for (arg, expr) in &e.args {
            let v = r.eval(t, expr, &scope)?;
            base_scope.vars.insert(arg.clone(), v);
        }
        let mut out = String::new();
        r.nodes(base, &base.body, &mut base_scope, &mut out, Some(&body)).map_err(|err| {
            let (line, col) = line_col(&t.lines, e.pos.0);
            err.frame(format!("extend {:?} ({}:{}:{})", e.file, t.name, line, col))
        })?;
        Ok(out)
    }
}

impl<'a> Render<'a> {
    fn lookup(&self, scope: &Scope, name: &str) -> Option<Value> {
        scope.vars.get(name).or_else(|| self.globals.get(name)).cloned()
    }

    fn nodes(&mut self, t: &Template, nodes: &[Node], scope: &mut Scope, out: &mut String, body: Option<&str>) -> Result<()> {
        for n in nodes {
            match n {
                Node::Text(s) => out.push_str(s),
                Node::Insert(e) => {
                    let v = self.eval(t, e, scope)?;
                    match &v {
                        Value::Html(h) => out.push_str(h),
                        other => match text_of(other) {
                            Some(s) => out.push_str(&escape(&s)),
                            None => {
                                let what = describe(e);
                                let msg = if matches!(other, Value::Null) {
                                    format!("{what} is null")
                                } else {
                                    format!("cannot insert {}; {what} is {}", other.kind(), describe_hint(other))
                                };
                                return Err(t.error(e.pos(), msg));
                            }
                        },
                    }
                }
                Node::If(branches) => {
                    for (cond, body_nodes) in branches {
                        let take = match cond {
                            Some(c) => self.eval(t, c, scope)?.truthy(),
                            None => true,
                        };
                        if take {
                            let mut inner = Scope { vars: scope.vars.clone() };
                            self.nodes(t, body_nodes, &mut inner, out, body)?;
                            break;
                        }
                    }
                }
                Node::For { index, var, list, body: body_nodes } => {
                    let items = match self.eval(t, list, scope)? {
                        Value::List(l) => l,
                        Value::Null => Rc::new(Vec::new()),
                        other => return Err(t.error(list.pos(), format!("for needs a list, got {}", other.kind()))),
                    };
                    for (i, item) in items.iter().enumerate() {
                        let mut inner = Scope { vars: scope.vars.clone() };
                        inner.vars.insert(var.clone(), item.clone());
                        if let Some(ix) = index {
                            inner.vars.insert(ix.clone(), Value::Num(i as i64));
                        }
                        self.nodes(t, body_nodes, &mut inner, out, body)?;
                    }
                }
                Node::Let { name, value, pos } => {
                    if self.lookup(scope, name).is_some() {
                        return Err(t.error(*pos, format!("{name} is already defined; let cannot redeclare a visible name")));
                    }
                    let v = self.eval(t, value, scope)?;
                    scope.vars.insert(name.clone(), v);
                }
                Node::Include { file, args, pos } => {
                    let partial = &self.theme.templates[file];
                    let mut inner = Scope { vars: HashMap::new() };
                    for (name, e) in args {
                        let v = self.eval(t, e, scope)?;
                        inner.vars.insert(name.clone(), v);
                    }
                    self.nodes(partial, &partial.body, &mut inner, out, None).map_err(|err| {
                        let (line, col) = line_col(&t.lines, pos.0);
                        err.frame(format!("include {file:?} ({}:{}:{})", t.name, line, col))
                    })?;
                }
                Node::Yield(pos) => match body {
                    Some(b) => out.push_str(b),
                    None => return Err(t.error(*pos, "yield outside a base")),
                },
            }
        }
        Ok(())
    }

    fn eval(&mut self, t: &Template, e: &Expr, scope: &Scope) -> Result<Value> {
        Ok(match e {
            Expr::Str(s, _) => Value::str(s.as_str()),
            Expr::Num(n, _) => Value::Num(*n),
            Expr::Bool(b, _) => Value::Bool(*b),
            Expr::Var(name, pos) => self.lookup(scope, name).ok_or_else(|| t.error(*pos, format!("undefined variable {name:?}")))?,
            Expr::Field(x, name, pos) => {
                let base = self.eval(t, x, scope)?;
                match &base {
                    Value::Object(o) => o.field(name).ok_or_else(|| t.error(*pos, format!("{} has no field {name:?}", o.kind())))?,
                    Value::Map(m) => m.get(name).cloned().unwrap_or(Value::Null),
                    Value::StrictMap(m) => m.get(name).cloned().ok_or_else(|| t.error(*pos, format!("no {name:?} in {}", describe(x))))?,
                    other => return Err(t.error(*pos, format!("{} has no field {name:?}", other.kind()))),
                }
            }
            Expr::Call(name, args, pos) => {
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.eval(t, a, scope)?);
                }
                self.call(name, &vals).map_err(|m| t.error(*pos, m))?
            }
            Expr::Not(x, _) => Value::Bool(!self.eval(t, x, scope)?.truthy()),
            Expr::Bin(op, l, r, pos) => {
                let lv = self.eval(t, l, scope)?;
                match op {
                    BinOp::Or => return if lv.truthy() { Ok(lv) } else { self.eval(t, r, scope) },
                    BinOp::And => return if !lv.truthy() { Ok(lv) } else { self.eval(t, r, scope) },
                    _ => {}
                }
                let rv = self.eval(t, r, scope)?;
                match op {
                    BinOp::Eq => Value::Bool(equal(&lv, &rv).map_err(|m| t.error(*pos, m))?),
                    BinOp::Ne => Value::Bool(!equal(&lv, &rv).map_err(|m| t.error(*pos, m))?),
                    BinOp::Add => match (&lv, &rv) {
                        (Value::Num(a), Value::Num(b)) => Value::Num(a.checked_add(*b).ok_or_else(|| t.error(*pos, "number too large"))?),
                        (Value::Str(a), Value::Str(b)) => Value::str(format!("{a}{b}")),
                        _ => return Err(t.error(*pos, format!("+ needs two numbers or two strings, got {} and {}", lv.kind(), rv.kind()))),
                    },
                    _ => unreachable!(),
                }
            }
        })
    }

    fn call(&mut self, name: &str, args: &[Value]) -> std::result::Result<Value, String> {
        let arity = |n: usize| -> std::result::Result<(), String> {
            if args.len() != n {
                return Err(format!("{name} takes {n} argument{}, got {}", if n == 1 { "" } else { "s" }, args.len()));
            }
            Ok(())
        };
        let list = |i: usize| -> std::result::Result<&Rc<Vec<Value>>, String> {
            match &args[i] {
                Value::List(l) => Ok(l),
                other => Err(format!("{name}: argument {} must be a list, got {}", i + 1, other.kind())),
            }
        };
        let string = |i: usize| -> std::result::Result<&str, String> {
            match &args[i] {
                Value::Str(s) => Ok(s),
                other => Err(format!("{name}: argument {} must be a string, got {}", i + 1, other.kind())),
            }
        };
        let number = |i: usize| -> std::result::Result<i64, String> {
            match &args[i] {
                Value::Num(n) => Ok(*n),
                other => Err(format!("{name}: argument {} must be a number, got {}", i + 1, other.kind())),
            }
        };
        Ok(match name {
            "len" => {
                arity(1)?;
                Value::Num(list(0)?.len() as i64)
            }
            "take" => {
                arity(2)?;
                let (l, n) = (list(0)?, number(1)?);
                if n < 0 {
                    return Err(format!("take: negative count {n}"));
                }
                Value::list(l.iter().take(n as usize).cloned().collect())
            }
            "last" => {
                arity(1)?;
                list(0)?.last().cloned().unwrap_or(Value::Null)
            }
            "has" => {
                arity(2)?;
                if matches!(args[1], Value::Null) {
                    return Err("has: argument 2 is null".into());
                }
                let mut found = false;
                for it in list(0)?.iter() {
                    if equal(it, &args[1]).map_err(|m| format!("has: {m}"))? {
                        found = true;
                        break;
                    }
                }
                Value::Bool(found)
            }
            "lower" => {
                arity(1)?;
                Value::str(string(0)?.to_lowercase())
            }
            "starts" => {
                arity(2)?;
                Value::Bool(string(0)?.starts_with(string(1)?))
            }
            "pad" => {
                arity(2)?;
                let (n, w) = (number(0)?, number(1)?);
                if !(0..=64).contains(&w) {
                    return Err(format!("pad: width {w} out of range"));
                }
                Value::str(format!("{:0width$}", n, width = w as usize))
            }
            "plural" => {
                arity(2)?;
                let (n, w) = (number(0)?, string(1)?);
                Value::str(if n == 1 { w.to_string() } else { format!("{w}s") })
            }
            "date" => {
                arity(2)?;
                match &args[0] {
                    Value::Date(d) => Value::str(d.format(string(1)?)),
                    other => return Err(format!("date: argument 1 must be a date, got {}", other.kind())),
                }
            }
            "url" => {
                arity(1)?;
                Value::str(self.host.url(string(0)?).map_err(|m| format!("url: {m}"))?)
            }
            other => return Err(format!("unknown function {other:?}")),
        })
    }
}

/// The source-like spelling of an expression, for messages.
fn describe(e: &Expr) -> String {
    match e {
        Expr::Str(s, _) => format!("{s:?}"),
        Expr::Num(n, _) => n.to_string(),
        Expr::Bool(b, _) => b.to_string(),
        Expr::Var(n, _) => n.clone(),
        Expr::Field(x, n, _) => format!("{}.{n}", describe(x)),
        Expr::Call(n, _, _) => format!("{n}(...)"),
        Expr::Bin(..) | Expr::Not(..) => "the expression".into(),
    }
}

fn describe_hint(v: &Value) -> &'static str {
    match v {
        Value::List(_) => "a list; loop over it",
        Value::Map(_) | Value::StrictMap(_) => "a map",
        Value::Bool(_) => "a bool",
        Value::Object(_) => "an object; insert one of its fields",
        _ => "not text",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoHost;
    impl Host for NoHost {
        fn url(&self, p: &str) -> std::result::Result<String, String> {
            Ok(format!("/{p}"))
        }
    }

    fn render(files: &[(&str, &str)], name: &str, vars: Vars) -> std::result::Result<String, String> {
        let files: Vec<(String, String)> = files.iter().map(|(n, s)| (format!("theme/{n}"), s.to_string())).collect();
        let theme = Theme::load(&files).map_err(|e| e.to_string())?;
        theme.render(name, &vars, &NoHost).map_err(|e| e.to_string())
    }

    #[test]
    fn inserts_and_lines() {
        let out = render(
            &[("p.html", "a\n  {% if x %}\n<b>{{ y }}</b>\n  {% end %}\nc {{ \"}}\" }} {# gone #}\n")],
            "p.html",
            Vars::from([("x".into(), Value::Bool(true)), ("y".into(), Value::str("<&>"))]),
        )
        .unwrap();
        assert_eq!(out, "a\n<b>&lt;&amp;&gt;</b>\nc }} \n");
    }

    #[test]
    fn extend_and_include() {
        let files = [
            ("base.html", "<title>{{ title }}</title>\n{% yield %}\n"),
            ("_row.html", "<li>{{ item }}{{ n }}</li>\n"),
            ("p.html", "{% extend \"base.html\" title = t + \"!\" %}\n<ul>\n{% for i, x in xs %}\n{% include \"_row.html\" item = x, n = i + 1 %}\n{% end %}\n</ul>\n"),
        ];
        let vars = Vars::from([("t".into(), Value::str("T")), ("xs".into(), Value::list(vec![Value::str("a"), Value::str("b")]))]);
        // t and xs are not globals: extend/include checks are about the target's free names only.
        let out = render(&files, "p.html", vars).unwrap();
        assert_eq!(out, "<title>T!</title>\n<ul>\n<li>a1</li>\n<li>b2</li>\n</ul>\n");
    }

    #[test]
    fn errors_name_the_place() {
        let e = render(&[("p.html", "x\n {{ nope }}")], "p.html", Vars::new()).unwrap_err();
        assert_eq!(e, "theme/p.html:2:5: undefined variable \"nope\"");
        let e = render(&[("p.html", "{{ v }}")], "p.html", Vars::from([("v".into(), Value::Null)])).unwrap_err();
        assert_eq!(e, "theme/p.html:1:4: v is null");
        let e = render(&[("p.html", "{% let a = 1 %}{% let a = 2 %}")], "p.html", Vars::new()).unwrap_err();
        assert!(e.contains("cannot redeclare"), "{e}");
        let e = render(&[("p.html", "{% include \"_r.html\" q = 1 %}"), ("_r.html", "{{ z }}")], "p.html", Vars::new()).unwrap_err();
        assert!(e.contains("does not read q"), "{e}");
        let e = render(&[("p.html", "{% include \"_r.html\" %}"), ("_r.html", "x{{ z }}")], "p.html", Vars::new()).unwrap_err();
        assert_eq!(e, "theme/p.html:1:1: \"_r.html\" reads z, which is not given");
        let e = render(&[("p.html", "{% if a %}{% else if nope %}{% end %}")], "p.html", Vars::from([("a".into(), Value::Null)])).unwrap_err();
        assert_eq!(e, "theme/p.html:1:22: undefined variable \"nope\"");
        let e = render(&[("p.html", "{{ \"a\\\u{e9}\" }}{{ 99999999999999999999 }}")], "p.html", Vars::new()).unwrap_err();
        assert!(e.starts_with("theme/p.html:1:"), "{e}");
    }

    #[test]
    fn operators() {
        let vars = Vars::from([("a".into(), Value::Null), ("b".into(), Value::str("B")), ("n".into(), Value::Num(2))]);
        let out = render(
            &[("p.html", "{{ a or b }}|{{ b and n }}|{% if not a %}T{% end %}|{{ n + 1 }}|{% if n == 2 %}E{% end %}|{{ \"x\" + b }}")],
            "p.html",
            vars,
        )
        .unwrap();
        assert_eq!(out, "B|2|T|3|E|xB");
        let e = render(&[("p.html", "{{ not a }}")], "p.html", Vars::from([("a".into(), Value::Null)])).unwrap_err();
        assert!(e.contains("cannot insert bool"), "{e}");
    }

    #[test]
    fn for_heads_and_scopes() {
        let vars = Vars::from([("xs".into(), Value::list(vec![Value::Num(1), Value::Num(2)]))]);
        let out = render(&[("p.html", "{% for i,\n x in xs %}{{ i }}:{{ x }} {% end %}")], "p.html", vars.clone()).unwrap();
        assert_eq!(out, "0:1 1:2 ");
        let e = render(&[("p.html", "{% for x, x in xs %}{% end %}")], "p.html", vars.clone()).unwrap_err();
        assert!(e.contains("same name"), "{e}");
        let e = render(&[("p.html", "{% if 1 %}{% end if %}")], "p.html", Vars::new()).unwrap_err();
        assert!(e.contains("end takes no keyword"), "{e}");
        let e = render(&[("p.html", "{% end %}")], "p.html", Vars::new()).unwrap_err();
        assert!(e.contains("unexpected end"), "{e}");
        // A partial's loop variable is not a free name.
        let out = render(&[("p.html", "{% include \"_l.html\" items = xs %}"), ("_l.html", "{% for p in items %}{{ p }}{% end %}")], "p.html", vars).unwrap();
        assert_eq!(out, "12");
    }
}
