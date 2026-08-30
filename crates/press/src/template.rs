//! The template language: HTML plus insert, repeat, choose and compose.
//! docs/templates.md is the definition. The engine knows nothing about
//! sites: pages, sections and terms arrive as objects behind a trait, and
//! `url()` and `sri()` are answered by a host.

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
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('Y') => out.push_str(&self.year.to_string()),
                Some('m') => out.push_str(&format!("{:02}", self.month)),
                Some('d') => out.push_str(&format!("{:02}", self.day)),
                Some('b') => out.push_str(&MONTHS[(self.month as usize - 1).min(11)][..3]),
                Some('B') => out.push_str(MONTHS[(self.month as usize - 1).min(11)]),
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
    /// Identity, for `==`.
    fn ident(&self) -> usize;
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
        (Value::List(x), Value::List(y)) => {
            if x.len() != y.len() {
                return Ok(false);
            }
            for (p, q) in x.iter().zip(y.iter()) {
                if !equal(p, q)? {
                    return Ok(false);
                }
            }
            true
        }
        (Value::Object(x), Value::Object(y)) => x.kind() == y.kind() && x.ident() == y.ident(),
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

/// What the site provides to templates beyond data.
pub trait Host {
    fn url(&self, path: &str) -> std::result::Result<String, String>;
    fn sri(&self, path: &str) -> std::result::Result<String, String>;
}

// --- syntax ---------------------------------------------------------------

#[derive(Clone, Copy)]
struct Pos(usize);

enum Expr {
    Str(String),
    Num(i64),
    Bool(bool),
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
            Expr::Str(_) | Expr::Num(_) | Expr::Bool(_) => Pos(0),
            Expr::Var(_, p) | Expr::Field(_, _, p) | Expr::Call(_, _, p) | Expr::Bin(_, _, _, p) | Expr::Not(_, p) => *p,
        }
    }
}

enum Node {
    Text(String),
    Insert(Expr, Pos),
    If(Vec<(Option<Expr>, Vec<Node>)>),
    For { index: Option<String>, var: String, list: Expr, body: Vec<Node>, pos: Pos },
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
    src: Rc<str>,
    lines: Vec<usize>,
    extend: Option<Extend>,
    body: Vec<Node>,
    yields: usize,
    reads: HashSet<String>,
}

impl Template {
    fn pos(&self, off: usize) -> (usize, usize) {
        let off = off.min(self.src.len());
        let line = self.lines.partition_point(|&s| s <= off) - 1;
        (line + 1, off - self.lines[line] + 1)
    }

    fn error(&self, at: Pos, msg: impl Into<String>) -> Error {
        let (line, col) = self.pos(at.0);
        Error::at(self.name.clone(), line, col, msg)
    }
}

/// A theme: every `theme/*.html` parsed once.
pub struct Theme {
    templates: HashMap<String, Rc<Template>>,
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
    Newline(usize),
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
                pieces.push(Piece::Newline(from + k));
                s = from + k + 1;
            }
        }
        if to > s {
            pieces.push(Piece::Text(s, to));
        }
    };
    let pos = |off: usize| {
        let line = lines.partition_point(|&s| s <= off) - 1;
        (line + 1, off - lines[line] + 1)
    };
    while i < b.len() {
        if b[i] == b'{' && i + 1 < b.len() && matches!(b[i + 1], b'{' | b'%' | b'#') {
            flush_text(&mut pieces, text_start, i);
            let kind = match b[i + 1] {
                b'{' => TagKind::Insert,
                b'%' => TagKind::Stmt,
                _ => TagKind::Comment,
            };
            let closer: &[u8] = match kind {
                TagKind::Insert => b"}}",
                TagKind::Stmt => b"%}",
                TagKind::Comment => b"#}",
            };
            let mut j = i + 2;
            let end = loop {
                if j + 1 >= b.len() + 1 || j >= b.len() {
                    let (line, col) = pos(i);
                    return Err(Error::at(name, line, col, "unclosed tag"));
                }
                // The closer counts only between tokens: not inside a string.
                if kind != TagKind::Comment && b[j] == b'"' {
                    j += 1;
                    while j < b.len() && b[j] != b'"' {
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
    let finish = |line: &mut Vec<Piece>, out: &mut Vec<Piece>, newline: Option<Piece>| {
        let has_stmt = line.iter().any(|p| matches!(p, Piece::Tag { kind: TagKind::Stmt | TagKind::Comment, .. }));
        let clean = line.iter().all(|p| match p {
            Piece::Text(a, z) => src[*a..*z].bytes().all(|c| c == b' ' || c == b'\t'),
            Piece::Tag { kind, .. } => *kind != TagKind::Insert,
            Piece::Newline(_) => true,
        });
        if has_stmt && clean {
            for p in line.drain(..) {
                if matches!(p, Piece::Tag { .. }) {
                    out.push(p);
                }
            }
        } else {
            out.append(line);
            if let Some(nl) = newline {
                out.push(nl);
            }
        }
    };
    for p in pieces {
        match p {
            Piece::Newline(_) => finish(&mut line, &mut out, Some(p)),
            other => line.push(other),
        }
    }
    finish(&mut line, &mut out, None);
    Ok(out)
}

// --- parsing --------------------------------------------------------------

struct Parser<'a> {
    name: &'a str,
    src: &'a str,
    lines: &'a [usize],
    pieces: Vec<Piece>,
    i: usize,
    reads: HashSet<String>,
    yields: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn error(&self, at: usize, msg: impl Into<String>) -> Error {
        let line = self.lines.partition_point(|&s| s <= at) - 1;
        Error::at(self.name, line + 1, at - self.lines[line] + 1, msg)
    }

    /// Statements up to an `end`/`else` keyword tag (returned) or the end of the file.
    fn nodes(&mut self, until_keyword: bool) -> Result<(Vec<Node>, Option<(String, usize, usize)>)> {
        let mut out = Vec::new();
        while self.i < self.pieces.len() {
            let p = &self.pieces[self.i];
            self.i += 1;
            match *p {
                Piece::Text(a, z) => out.push(Node::Text(self.src[a..z].to_string())),
                Piece::Newline(_) => out.push(Node::Text("\n".into())),
                Piece::Tag { kind: TagKind::Comment, .. } => {}
                Piece::Tag { kind: TagKind::Insert, inner, at } => {
                    let e = self.expr_in(inner)?;
                    out.push(Node::Insert(e, Pos(at)));
                }
                Piece::Tag { kind: TagKind::Stmt, inner, at } => {
                    let text = self.src[inner.0..inner.1].trim();
                    let (kw, rest) = match text.find(|c: char| c.is_whitespace()) {
                        Some(k) => (&text[..k], text[k..].trim_start()),
                        None => (text, ""),
                    };
                    let rest_off = inner.0 + (self.src[inner.0..inner.1].find(rest).unwrap_or(0));
                    let rest_off = if rest.is_empty() { inner.1 } else { rest_off };
                    match kw {
                        "end" | "else" => {
                            if !until_keyword {
                                return Err(self.error(at, format!("unexpected {kw}")));
                            }
                            return Ok((out, Some((kw.to_string(), at, rest_off))));
                        }
                        "if" => {
                            self.depth += 1;
                            let mut branches = Vec::new();
                            let mut cond = Some(self.expr_at(rest, rest_off)?);
                            loop {
                                let (body, stop) = self.nodes(true)?;
                                branches.push((cond.take(), body));
                                match stop {
                                    Some((k, _, _)) if k == "end" => break,
                                    Some((_, else_at, else_rest)) => {
                                        let else_text = self.src[else_rest..].trim_start();
                                        let tag_text = self.stmt_text(self.i - 1);
                                        let after_else = tag_text.trim().strip_prefix("else").unwrap_or("").trim();
                                        if after_else.is_empty() {
                                            let (body, stop) = self.nodes(true)?;
                                            branches.push((None, body));
                                            match stop {
                                                Some((k, _, _)) if k == "end" => break,
                                                Some((_, at, _)) => return Err(self.error(at, "only one else per if")),
                                                None => return Err(self.error(else_at, "unclosed if")),
                                            }
                                        } else if let Some(c) = after_else.strip_prefix("if").filter(|r| r.starts_with(|c: char| c.is_whitespace())) {
                                            let c_off = self.src.len() - else_text.len() + (else_text.len() - c.trim_start().len());
                                            cond = Some(self.expr_at(c.trim(), c_off.min(self.src.len()))?);
                                        } else {
                                            return Err(self.error(else_at, "expected else or else if"));
                                        }
                                    }
                                    None => return Err(self.error(at, "unclosed if")),
                                }
                            }
                            self.depth -= 1;
                            out.push(Node::If(branches));
                        }
                        "for" => {
                            let Some((head, list)) = rest.split_once(" in ") else {
                                return Err(self.error(at, "expected: for x in list"));
                            };
                            let names: Vec<&str> = head.split(',').map(str::trim).collect();
                            let (index, var) = match names.as_slice() {
                                [v] => (None, v.to_string()),
                                [i, v] => (Some(i.to_string()), v.to_string()),
                                _ => return Err(self.error(at, "expected: for x in list or for i, x in list")),
                            };
                            for n in [&var].into_iter().chain(index.iter()) {
                                self.check_name(n, at)?;
                            }
                            let list_off = rest_off + rest.find(" in ").unwrap() + 4;
                            let list = self.expr_at(list.trim(), list_off + (list.len() - list.trim_start().len()))?;
                            self.depth += 1;
                            let (body, stop) = self.nodes(true)?;
                            self.depth -= 1;
                            match stop {
                                Some((k, _, _)) if k == "end" => {}
                                Some((_, at, _)) => return Err(self.error(at, "else is not allowed in for")),
                                None => return Err(self.error(at, "unclosed for")),
                            }
                            out.push(Node::For { index, var, list, body, pos: Pos(at) });
                        }
                        "let" => {
                            let Some((name, value)) = rest.split_once('=') else {
                                return Err(self.error(at, "expected: let name = value"));
                            };
                            let name = name.trim();
                            self.check_name(name, at)?;
                            let v_off = rest_off + rest.find('=').unwrap() + 1;
                            let value = self.expr_at(value.trim(), v_off + (value.len() - value.trim_start().len()))?;
                            out.push(Node::Let { name: name.to_string(), value, pos: Pos(at) });
                        }
                        "include" => {
                            let (file, args) = self.file_and_args(rest, rest_off, at)?;
                            if !file.starts_with('_') {
                                return Err(self.error(at, format!("include names a partial (_*.html), not {file:?}")));
                            }
                            out.push(Node::Include { file, args, pos: Pos(at) });
                        }
                        "extend" => return Err(self.error(at, "extend must be the first statement of a page template")),
                        "yield" => {
                            if !rest.is_empty() {
                                return Err(self.error(at, "yield takes no arguments"));
                            }
                            self.yields += 1;
                            out.push(Node::Yield(Pos(at)));
                        }
                        other => return Err(self.error(at, format!("unknown statement {other:?}"))),
                    }
                }
            }
        }
        Ok((out, None))
    }

    fn stmt_text(&self, idx: usize) -> &'a str {
        match self.pieces[idx] {
            Piece::Tag { inner, .. } => &self.src[inner.0..inner.1],
            _ => "",
        }
    }

    fn check_name(&self, name: &str, at: usize) -> Result<()> {
        if !is_name(name) {
            return Err(self.error(at, format!("{name:?} is not a name")));
        }
        if KEYWORDS.contains(&name) {
            return Err(self.error(at, format!("{name} is a keyword")));
        }
        Ok(())
    }

    /// `"file.html" name = expr, name = expr` for include and extend.
    fn file_and_args(&mut self, rest: &str, rest_off: usize, at: usize) -> Result<(String, Vec<(String, Expr)>)> {
        let mut lx = Lexer::new(rest, rest_off);
        let file = match lx.next_token()? {
            Some(Tok::Str(s, _)) => s,
            _ => return Err(self.error(at, "expected a quoted file name")),
        };
        let mut args = Vec::new();
        let mut seen = HashSet::new();
        loop {
            match lx.next_token()? {
                None => break,
                Some(Tok::Name(name, p)) => {
                    if !seen.insert(name.clone()) {
                        return Err(self.error(p, format!("argument {name} given twice")));
                    }
                    self.check_name(&name, p)?;
                    match lx.next_token()? {
                        Some(Tok::Punct("=", _)) => {}
                        _ => return Err(self.error(p, format!("expected = after {name}"))),
                    }
                    let (e, next) = self.parse_expr_tokens(&mut lx)?;
                    args.push((name, e));
                    match next {
                        None => break,
                        Some(Tok::Punct(",", _)) => continue,
                        Some(t) => return Err(self.error(t.pos(), "expected , between arguments")),
                    }
                }
                Some(t) => return Err(self.error(t.pos(), "expected an argument name")),
            }
        }
        Ok((file, args))
    }

    fn expr_in(&mut self, inner: (usize, usize)) -> Result<Expr> {
        let text = &self.src[inner.0..inner.1];
        self.expr_at(text, inner.0)
    }

    fn expr_at(&mut self, text: &str, off: usize) -> Result<Expr> {
        let mut lx = Lexer::new(text, off);
        let (e, next) = self.parse_expr_tokens(&mut lx)?;
        if let Some(t) = next {
            return Err(self.error(t.pos(), "unexpected text after expression"));
        }
        Ok(e)
    }

    /// Parses one expression and returns the token that follows it.
    fn parse_expr_tokens(&mut self, lx: &mut Lexer<'_>) -> Result<(Expr, Option<Tok>)> {
        let first = lx.next_token().map_err(|e| self.error(e.col, e.msg))?;
        let mut ep = ExprParser { p: self, lx, cur: first };
        let e = ep.or()?;
        let next = ep.cur.take();
        Ok((e, next))
    }
}

const KEYWORDS: &[&str] = &["if", "else", "end", "for", "in", "let", "include", "extend", "yield", "and", "or", "not", "true", "false"];

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

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

struct Lexer<'a> {
    s: &'a str,
    off: usize,
    i: usize,
}

impl<'a> Lexer<'a> {
    fn new(s: &'a str, off: usize) -> Lexer<'a> {
        Lexer { s, off, i: 0 }
    }

    fn next_token(&mut self) -> Result<Option<Tok>> {
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
            let n: i64 = self.s[start..self.i].parse().map_err(|_| Error::new("", "number too large"))?;
            return Ok(Some(Tok::Num(n, at)));
        }
        if c == b'"' {
            self.i += 1;
            let mut out = String::new();
            loop {
                if self.i >= b.len() {
                    return Err(Error::at("", 0, at, "unterminated string"));
                }
                match b[self.i] {
                    b'"' => {
                        self.i += 1;
                        break;
                    }
                    b'\\' if self.i + 1 < b.len() => {
                        out.push(b[self.i + 1] as char);
                        self.i += 2;
                    }
                    _ => {
                        let ch = self.s[self.i..].chars().next().unwrap();
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
        Err(Error::at("", 0, at, format!("unexpected {:?}", c as char)))
    }
}

struct ExprParser<'a, 'b, 'c> {
    p: &'b mut Parser<'a>,
    lx: &'b mut Lexer<'c>,
    cur: Option<Tok>,
}

impl<'a, 'b, 'c> ExprParser<'a, 'b, 'c> {
    fn advance(&mut self) -> Result<()> {
        self.cur = self.lx.next_token().map_err(|e| self.p.error(e.col, e.msg))?;
        Ok(())
    }

    fn is_name(&self, kw: &str) -> bool {
        matches!(&self.cur, Some(Tok::Name(n, _)) if n == kw)
    }

    fn is_punct(&self, p: &str) -> bool {
        matches!(&self.cur, Some(Tok::Punct(q, _)) if *q == p)
    }

    fn or(&mut self) -> Result<Expr> {
        let mut l = self.and()?;
        while self.is_name("or") {
            let at = self.cur.as_ref().unwrap().pos();
            self.advance()?;
            let r = self.and()?;
            l = Expr::Bin(BinOp::Or, Box::new(l), Box::new(r), Pos(at));
        }
        Ok(l)
    }

    fn and(&mut self) -> Result<Expr> {
        let mut l = self.not()?;
        while self.is_name("and") {
            let at = self.cur.as_ref().unwrap().pos();
            self.advance()?;
            let r = self.not()?;
            l = Expr::Bin(BinOp::And, Box::new(l), Box::new(r), Pos(at));
        }
        Ok(l)
    }

    fn not(&mut self) -> Result<Expr> {
        if self.is_name("not") {
            let at = self.cur.as_ref().unwrap().pos();
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
            let at = self.cur.as_ref().unwrap().pos();
            self.advance()?;
            let r = self.add()?;
            l = Expr::Bin(op, Box::new(l), Box::new(r), Pos(at));
        }
    }

    fn add(&mut self) -> Result<Expr> {
        let mut l = self.postfix()?;
        while self.is_punct("+") {
            let at = self.cur.as_ref().unwrap().pos();
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
            Some(Tok::Str(s, _)) => {
                self.advance()?;
                Ok(Expr::Str(s))
            }
            Some(Tok::Num(n, _)) => {
                self.advance()?;
                Ok(Expr::Num(n))
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
                    "true" => return Ok(Expr::Bool(true)),
                    "false" => return Ok(Expr::Bool(false)),
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
                self.p.reads.insert(name.clone());
                Ok(Expr::Var(name, Pos(at)))
            }
            Some(t) => Err(self.p.error(t.pos(), "expected a value")),
            None => Err(self.p.error(self.lx.off + self.lx.i, "expected a value")),
        }
    }
}

fn parse_template(name: &str, src: &str) -> Result<Template> {
    let src: Rc<str> = src
        .strip_prefix('\u{feff}')
        .unwrap_or(src)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .into();
    let mut lines = vec![0];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            lines.push(i + 1);
        }
    }
    let pieces = lex(name, &src, &lines)?;
    let mut p = Parser { name, src: &src, lines: &lines, pieces, i: 0, reads: HashSet::new(), yields: 0, depth: 0 };
    // extend: the first statement, after whitespace and comments.
    let mut extend = None;
    let mut k = 0;
    while k < p.pieces.len() {
        match p.pieces[k] {
            Piece::Newline(_) => k += 1,
            Piece::Text(a, z) if src[a..z].trim().is_empty() => k += 1,
            Piece::Tag { kind: TagKind::Comment, .. } => k += 1,
            Piece::Tag { kind: TagKind::Stmt, inner, at } => {
                let text = src[inner.0..inner.1].trim_start();
                if let Some(rest) = text.strip_prefix("extend").filter(|r| r.starts_with(|c: char| c.is_whitespace())) {
                    let rest_off = inner.0 + (src[inner.0..inner.1].len() - rest.len());
                    let (file, args) = p.file_and_args(rest.trim(), rest_off + (rest.len() - rest.trim_start().len()), at)?;
                    extend = Some(Extend { file, args, pos: Pos(at) });
                    p.i = k + 1;
                }
                break;
            }
            _ => break,
        }
    }
    let (body, stop) = p.nodes(false)?;
    if let Some((_, at, _)) = stop {
        return Err(p.error(at, "unexpected end"));
    }
    let (reads, yields) = (p.reads, p.yields);
    Ok(Template { name: name.to_string(), src, lines, extend, body, yields, reads })
}

// --- the theme and rendering ------------------------------------------------

/// The variables a render starts with: the globals plus the page-kind value.
pub type Vars = Vec<(String, Value)>;

struct Scope {
    vars: HashMap<String, Value>,
}

struct Render<'a> {
    theme: &'a Theme,
    host: &'a dyn Host,
    globals: &'a HashMap<String, Value>,
    depth: usize,
}

impl Theme {
    /// Parses every file; `files` maps `theme/name.html` to its text.
    pub fn load(files: &[(String, String)]) -> Result<Theme> {
        let mut templates = HashMap::new();
        for (name, src) in files {
            let short = name.rsplit('/').next().unwrap_or(name).to_string();
            templates.insert(short, Rc::new(parse_template(name, src)?));
        }
        let theme = Theme { templates };
        theme.check()?;
        Ok(theme)
    }

    /// The static rules: where extend and yield may appear, includes exist
    /// and read their arguments, no include cycles.
    fn check(&self) -> Result<()> {
        let bases: HashSet<&str> = self.templates.values().filter_map(|t| t.extend.as_ref().map(|e| e.file.as_str())).collect();
        for t in self.templates.values() {
            let short = t.name.rsplit('/').next().unwrap_or(&t.name);
            let is_partial = short.starts_with('_');
            if let Some(e) = &t.extend {
                if is_partial {
                    return Err(t.error(e.pos, "a partial cannot extend"));
                }
                let Some(base) = self.templates.get(&e.file) else {
                    return Err(t.error(e.pos, format!("no template {:?} to extend", e.file)));
                };
                if base.extend.is_some() {
                    return Err(t.error(e.pos, format!("{:?} extends a template itself; only one level", e.file)));
                }
                if base.yields != 1 {
                    return Err(t.error(e.pos, format!("{:?} must contain exactly one yield, it has {}", e.file, base.yields)));
                }
                for (name, _) in &e.args {
                    if !base.reads.contains(name) {
                        return Err(t.error(e.pos, format!("{:?} does not read {name}", e.file)));
                    }
                }
            }
            if t.yields > 0 && !bases.contains(short) {
                let at = find_yield(&t.body).unwrap_or(Pos(0));
                return Err(t.error(at, "yield is only allowed in a template that another extends"));
            }
            self.check_includes(t, &t.body, &mut vec![short.to_string()])?;
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
                    for (name, _) in args {
                        if !partial.reads.contains(name) {
                            return Err(t.error(*pos, format!("{file:?} does not read {name}")));
                        }
                    }
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
    pub fn render(&self, name: &str, globals: &Vars, host: &dyn Host) -> Result<String> {
        let t = self.templates.get(name).ok_or_else(|| Error::new(format!("theme/{name}"), "no such template"))?;
        let globals: HashMap<String, Value> = globals.iter().cloned().collect();
        let mut r = Render { theme: self, host, globals: &globals, depth: 0 };
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
            let (line, col) = t.pos(e.pos.0);
            err.frame(format!("extend {:?} ({}:{}:{})", e.file, t.name, line, col))
        })?;
        Ok(out)
    }
}

fn find_yield(nodes: &[Node]) -> Option<Pos> {
    for n in nodes {
        match n {
            Node::Yield(p) => return Some(*p),
            Node::If(branches) => {
                for (_, body) in branches {
                    if let Some(p) = find_yield(body) {
                        return Some(p);
                    }
                }
            }
            Node::For { body, .. } => {
                if let Some(p) = find_yield(body) {
                    return Some(p);
                }
            }
            _ => {}
        }
    }
    None
}

impl<'a> Render<'a> {
    fn lookup(&self, scope: &Scope, name: &str) -> Option<Value> {
        scope.vars.get(name).or_else(|| self.globals.get(name)).cloned()
    }

    fn nodes(&mut self, t: &Template, nodes: &[Node], scope: &mut Scope, out: &mut String, body: Option<&str>) -> Result<()> {
        for n in nodes {
            match n {
                Node::Text(s) => out.push_str(s),
                Node::Insert(e, pos) => {
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
                                return Err(t.error(*pos, msg));
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
                Node::For { index, var, list, body: body_nodes, pos } => {
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
                        self.nodes(t, body_nodes, &mut inner, out, body).map_err(|e| e)?;
                    }
                    let _ = pos;
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
                    self.depth += 1;
                    if self.depth > 100 {
                        return Err(t.error(*pos, "include depth exceeds 100"));
                    }
                    let r = self.nodes(partial, &partial.body, &mut inner, out, None);
                    self.depth -= 1;
                    r.map_err(|err| {
                        let (line, col) = t.pos(pos.0);
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
            Expr::Str(s) => Value::str(s.as_str()),
            Expr::Num(n) => Value::Num(*n),
            Expr::Bool(b) => Value::Bool(*b),
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
                        (Value::Num(a), Value::Num(b)) => {
                            Value::Num(a.checked_add(*b).ok_or_else(|| t.error(*pos, "number too large"))?)
                        }
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
                let l = list(0)?;
                let mut found = false;
                for it in l.iter() {
                    if equal(it, &args[1])? {
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
                if w < 0 || w > 64 {
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
                Value::str(self.host.url(string(0)?)?)
            }
            "sri" => {
                arity(1)?;
                Value::str(self.host.sri(string(0)?)?)
            }
            other => return Err(format!("unknown function {other:?}")),
        })
    }
}

/// The source-like spelling of an expression, for messages.
fn describe(e: &Expr) -> String {
    match e {
        Expr::Str(s) => format!("{s:?}"),
        Expr::Num(n) => n.to_string(),
        Expr::Bool(b) => b.to_string(),
        Expr::Var(n, _) => n.clone(),
        Expr::Field(x, n, _) => format!("{}.{n}", describe(x)),
        Expr::Call(n, _, _) => format!("{n}(...)"),
        Expr::Bin(_, _, _, _) | Expr::Not(_, _) => "the expression".into(),
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
        fn sri(&self, _: &str) -> std::result::Result<String, String> {
            Ok("sha384-x".into())
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
            vec![("x".into(), Value::Bool(true)), ("y".into(), Value::str("<&>"))],
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
        let out = render(&files, "p.html", vec![("t".into(), Value::str("T")), ("xs".into(), Value::list(vec![Value::str("a"), Value::str("b")]))]).unwrap();
        assert_eq!(out, "<title>T!</title>\n<ul>\n<li>a1</li>\n<li>b2</li>\n</ul>\n");
    }

    #[test]
    fn errors_name_the_place() {
        let e = render(&[("p.html", "x\n {{ nope }}")], "p.html", vec![]).unwrap_err();
        assert_eq!(e, "theme/p.html:2:5: undefined variable \"nope\"");
        let e = render(&[("p.html", "{{ v }}")], "p.html", vec![("v".into(), Value::Null)]).unwrap_err();
        assert_eq!(e, "theme/p.html:1:1: v is null");
        let e = render(&[("p.html", "{% let a = 1 %}{% let a = 2 %}")], "p.html", vec![]).unwrap_err();
        assert!(e.contains("cannot redeclare"), "{e}");
        let e = render(&[("p.html", "{% include \"_r.html\" q = 1 %}"), ("_r.html", "{{ z }}")], "p.html", vec![]).unwrap_err();
        assert!(e.contains("does not read q"), "{e}");
        let e = render(&[("p.html", "{% include \"_r.html\" %}"), ("_r.html", "x{{ z }}")], "p.html", vec![]).unwrap_err();
        assert_eq!(e, "theme/_r.html:1:5: undefined variable \"z\"\n  in include \"_r.html\" (theme/p.html:1:1)");
    }

    #[test]
    fn operators() {
        let vars: Vars = vec![("a".into(), Value::Null), ("b".into(), Value::str("B")), ("n".into(), Value::Num(2))];
        let out = render(
            &[("p.html", "{{ a or b }}|{{ b and n }}|{% if not a %}T{% end %}|{{ n + 1 }}|{% if n == 2 %}E{% end %}|{{ \"x\" + b }}")],
            "p.html",
            vars,
        )
        .unwrap();
        assert_eq!(out, "B|2|T|3|E|xB");
        let e = render(&[("p.html", "{{ not a }}")], "p.html", vec![("a".into(), Value::Null)]).unwrap_err();
        assert!(e.contains("cannot insert bool"), "{e}");
    }
}
