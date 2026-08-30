//! A scannerless recursive-descent parser over the source bytes. CSS-like
//! syntax is easiest to read character by character: whether `a:hover` is a
//! selector or a declaration depends on what follows it, and selectors,
//! at-rule preludes and custom property values are raw text with #{...}
//! holes rather than tokens.

use std::rc::Rc;

use crate::ast::*;
use crate::source::{Error, Result, Source, Span};
use crate::value::Value;

const MAX_NESTING: usize = 500;

pub struct Parser {
    src: Rc<Source>,
    s: Vec<u8>,
    i: usize,
    depth: usize, // block nesting, to keep @use at the top level
    nest: usize,  // every kind of nesting, against stack overflow
}

pub fn parse(src: &Rc<Source>) -> Result<Vec<Stmt>> {
    let mut p = Parser::new(src);
    let stmts = p.stmts()?;
    if !p.eof() {
        return Err(p.fail(p.i, format!("unexpected {:?}", p.s[p.i] as char)));
    }
    Ok(stmts)
}

/// Parses a parameter list on its own, for builtin signatures.
pub fn parse_params(sig: &str) -> Vec<Param> {
    let src = Source::new("builtin", sig);
    let mut p = Parser::new(&src);
    p.params().expect("builtin signature")
}

pub fn is_space(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\n'
}

fn is_digit(c: u8) -> bool {
    c.is_ascii_digit()
}

fn is_alpha(c: u8) -> bool {
    c.is_ascii_alphabetic()
}

/// Whether c can continue an identifier.
pub fn is_word_char(c: u8) -> bool {
    is_alpha(c) || is_digit(c) || c == b'_' || c == b'-' || c >= 0x80
}

fn is_hex_or_wild(c: u8) -> bool {
    c.is_ascii_hexdigit() || c == b'?'
}

/// The byte length of the UTF-8 character starting with b.
fn char_len(b: u8) -> usize {
    match b {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

impl Parser {
    fn new(src: &Rc<Source>) -> Parser {
        Parser { src: src.clone(), s: src.text.as_bytes().to_vec(), i: 0, depth: 0, nest: 0 }
    }

    fn fail(&self, off: usize, msg: impl Into<String>) -> Error {
        Span { src: self.src.clone(), a: off, b: off }.error(msg)
    }

    fn span(&self, start: usize) -> Span {
        Span { src: self.src.clone(), a: start, b: self.i }
    }

    fn eof(&self) -> bool {
        self.i >= self.s.len()
    }

    fn peek(&self) -> u8 {
        self.s.get(self.i).copied().unwrap_or(0)
    }

    fn peek_at(&self, n: usize) -> u8 {
        self.s.get(self.i + n).copied().unwrap_or(0)
    }

    fn at(&self, lit: &str) -> bool {
        self.s[self.i..].starts_with(lit.as_bytes())
    }

    fn slice(&self, a: usize, b: usize) -> String {
        text(&self.s[a..b])
    }

    fn expect(&mut self, c: u8) -> Result<()> {
        self.ws();
        if self.peek() != c {
            return Err(self.fail(self.i, format!("expected {:?}", c as char)));
        }
        self.i += 1;
        Ok(())
    }

    /// Bounds the nesting of blocks, parentheses, brackets and
    /// interpolation; a million open parentheses must be an error, not a
    /// crash. The error points at the opening token, at off.
    fn enter(&mut self, off: usize) -> Result<()> {
        self.nest += 1;
        if self.nest > MAX_NESTING {
            return Err(self.fail(off, format!("nesting deeper than {MAX_NESTING} levels")));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.nest -= 1;
    }

    /// Skips whitespace and // comments; reports whether anything was
    /// skipped. Block comments are left alone so statement parsing can keep
    /// them.
    fn space(&mut self) -> bool {
        let start = self.i;
        while !self.eof() {
            let c = self.s[self.i];
            if is_space(c) {
                self.i += 1;
            } else if c == b'/' && self.peek_at(1) == b'/' {
                while !self.eof() && self.s[self.i] != b'\n' {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
        self.i > start
    }

    /// Skips whitespace and every kind of comment.
    fn ws(&mut self) -> bool {
        let start = self.i;
        loop {
            self.space();
            if !self.at("/*") {
                return self.i > start;
            }
            // An unterminated comment here surfaces where it is used.
            if self.block_comment().is_err() {
                return self.i > start;
            }
        }
    }

    fn block_comment(&mut self) -> Result<String> {
        let start = self.i;
        match find(&self.s[self.i..], b"*/") {
            Some(end) => {
                self.i += end + 2;
                Ok(self.slice(start, self.i))
            }
            None => Err(self.fail(start, "unterminated comment")),
        }
    }

    // --- statements ---------------------------------------------------------

    /// Statements up to a closing brace or the end of input.
    fn stmts(&mut self) -> Result<Vec<Stmt>> {
        let mut out = Vec::new();
        loop {
            self.space();
            if self.eof() || self.peek() == b'}' {
                return Ok(out);
            }
            if self.peek() == b';' {
                self.i += 1;
                continue;
            }
            out.push(self.stmt()?);
        }
    }

    fn block(&mut self) -> Result<Vec<Stmt>> {
        self.ws();
        let open = self.i;
        self.expect(b'{')?;
        self.enter(open)?;
        self.depth += 1;
        let body = self.stmts()?;
        self.depth -= 1;
        self.leave();
        if self.eof() {
            return Err(self.fail(open, "unterminated block"));
        }
        self.expect(b'}')?;
        Ok(body)
    }

    fn stmt(&mut self) -> Result<Stmt> {
        let start = self.i;
        if self.at("/*") {
            let text = self.block_comment()?;
            return Ok(Stmt::Comment { span: self.span(start), text });
        }
        match self.peek() {
            b'@' => return self.at_rule(),
            b'$' => return self.assign(),
            _ => {}
        }
        if self.at("--") || !self.block_ahead() {
            return self.decl();
        }
        let selector = self.raw("{", false)?;
        self.check_selector(&selector, start)?;
        let body = self.block()?;
        Ok(Stmt::Rule { span: self.span(start), selector, body })
    }

    /// Whether the statement at the cursor opens a block: scans to the first
    /// ; { or } outside strings, brackets and #{...}.
    fn block_ahead(&self) -> bool {
        let s = &self.s;
        let mut depth = 0i32;
        let mut i = self.i;
        while i < s.len() {
            match s[i] {
                b'\\' => i += 1,
                b'"' | b'\'' => i = self.skip_string(i),
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth -= 1,
                b'#' if i + 1 < s.len() && s[i + 1] == b'{' => {
                    depth += 1;
                    i += 1;
                }
                b'{' if depth == 0 => return true,
                b'}' => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                b';' if depth == 0 => return false,
                b'/' if i + 1 < s.len() && s[i + 1] == b'/' => {
                    while i < s.len() && s[i] != b'\n' {
                        i += 1;
                    }
                }
                b'/' if i + 1 < s.len() && s[i + 1] == b'*' => {
                    if let Some(end) = find(&s[i..], b"*/") {
                        i += end + 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        false
    }

    /// The index of the closing quote of the string opening at i; for an
    /// unterminated string, the index of the newline or the end of input,
    /// which callers can tell apart from a quote.
    fn skip_string(&self, mut i: usize) -> usize {
        let q = self.s[i];
        i += 1;
        while i < self.s.len() {
            match self.s[i] {
                b'\\' => i += 1,
                c if c == q || c == b'\n' => return i,
                _ => {}
            }
            i += 1;
        }
        i
    }

    /// Rejects the two things that parse as a selector but were never meant
    /// as one: a declaration that lost its semicolon, and Sass's `&-suffix`,
    /// which CSS Nesting does not have.
    fn check_selector(&self, sel: &Interp, start: usize) -> Result<()> {
        for part in sel {
            let Part::Text(t) = part else { continue };
            let t = t.as_bytes();
            let mut depth = 0i32;
            let mut i = 0;
            while i < t.len() {
                match t[i] {
                    b'\\' => i += 1,
                    q @ (b'"' | b'\'') => {
                        i += 1;
                        while i < t.len() && t[i] != q {
                            if t[i] == b'\\' {
                                i += 1;
                            }
                            i += 1;
                        }
                    }
                    b'(' | b'[' => depth += 1,
                    b')' | b']' => depth -= 1,
                    // No selector ends with a colon or has a space after one.
                    b':' if depth == 0 && (i + 1 == t.len() || t[i + 1] == b' ') => {
                        return Err(self.fail(start, "unexpected '{' (missing ';' after a declaration?)"));
                    }
                    b'&' if i + 1 < t.len() && (is_word_char(t[i + 1]) || t[i + 1] == b'\\') => {
                        return Err(self.fail(
                            start,
                            format!("&{} is not CSS: nesting cannot build a selector name, write it flat", text(&t[i + 1..])),
                        ));
                    }
                    _ => {}
                }
                i += 1;
            }
        }
        Ok(())
    }

    fn decl(&mut self) -> Result<Stmt> {
        let start = self.i;
        let name = self.raw(":", false)?;
        let first = match name.first() {
            Some(Part::Text(t)) => t.clone(),
            Some(Part::Hole(_)) => String::new(),
            None => return Err(self.fail(start, "expected a declaration or a rule")),
        };
        if let Some(&c) = first.as_bytes().first() {
            if !(is_alpha(c) || c == b'_' || c == b'-' || c == b'\\' || c >= 0x80) {
                // `*zoom: 1` and friends are CSS error-recovery hacks, not CSS.
                return Err(self.fail(start, format!("invalid declaration name {first:?}")));
            }
        }
        self.expect(b':')?;
        let (value, raw, important);
        if first.starts_with("--") {
            raw = Some(self.raw(";}", true)?);
            value = None;
            important = String::new();
        } else {
            self.ws();
            value = Some(self.expr()?);
            raw = None;
            self.ws();
            important = if self.peek() == b'!' {
                let bang = self.i;
                self.i += 1;
                self.ws();
                if !self.word().eq_ignore_ascii_case("important") {
                    return Err(self.fail(bang, "expected !important"));
                }
                self.slice(bang, self.i)
            } else {
                String::new()
            };
        }
        let span = self.span(start);
        self.end_stmt()?;
        Ok(Stmt::Decl { span, name, value, raw, important })
    }

    /// Consumes the ; after a statement, or accepts a following } or the
    /// end of input in its place.
    fn end_stmt(&mut self) -> Result<()> {
        self.ws();
        match self.peek() {
            b';' => {
                self.i += 1;
                Ok(())
            }
            b'}' | 0 => Ok(()),
            _ => Err(self.fail(self.i, "expected ';'")),
        }
    }

    /// Raw text with #{...} holes up to one of the stop characters at bracket
    /// depth zero. Strings are copied verbatim (but still interpolated),
    /// comments are dropped, whitespace runs become one space. With braces
    /// set, balanced { } are part of the text (custom property values).
    fn raw(&mut self, stops: &str, braces: bool) -> Result<Interp> {
        let mut out: Interp = Vec::new();
        let mut text: Vec<u8> = Vec::new();
        let mut depth = 0i32;
        let mut pending_space = false;
        macro_rules! flush {
            () => {
                if !text.is_empty() {
                    out.push(Part::Text(String::from_utf8_lossy(&text).into_owned()));
                    text.clear();
                }
            };
        }
        macro_rules! put {
            ($bytes:expr) => {{
                if pending_space {
                    text.push(b' ');
                    pending_space = false;
                }
                text.extend_from_slice($bytes);
            }};
        }
        while !self.eof() {
            let c = self.s[self.i];
            if depth == 0 && stops.as_bytes().contains(&c) || c == b'}' && (!braces || depth == 0) {
                flush!();
                return Ok(out);
            }
            if is_space(c) {
                if !text.is_empty() || !out.is_empty() {
                    pending_space = true;
                }
                self.i += 1;
            } else if c == b'/' && self.peek_at(1) == b'/' {
                // The comment ends at a newline, and that newline is whitespace.
                self.space();
                if !text.is_empty() || !out.is_empty() {
                    pending_space = true;
                }
            } else if c == b'/' && self.peek_at(1) == b'*' {
                self.block_comment()?;
            } else if c == b'"' || c == b'\'' {
                // A quoted string inside raw text, interpolated: [data-x="#{$y}"].
                let q = c;
                let start = self.i;
                put!(&[q]);
                self.i += 1;
                loop {
                    if self.eof() || self.peek() == b'\n' {
                        return Err(self.fail(start, "unterminated string"));
                    }
                    let c = self.s[self.i];
                    if c == q {
                        put!(&[q]);
                        self.i += 1;
                        break;
                    } else if c == b'\\' && self.i + 1 < self.s.len() {
                        let n = 1 + char_len(self.s[self.i + 1]);
                        put!(&self.s[self.i..self.i + n].to_vec());
                        self.i += n;
                    } else if c == b'#' && self.peek_at(1) == b'{' {
                        flush!();
                        out.push(Part::Hole(self.hole()?));
                    } else {
                        put!(&[c]);
                        self.i += 1;
                    }
                }
            } else if c == b'#' && self.peek_at(1) == b'{' {
                if pending_space {
                    put!(b"");
                }
                flush!();
                out.push(Part::Hole(self.hole()?));
            } else if c == b'\\' && self.i + 1 < self.s.len() {
                let n = 1 + char_len(self.s[self.i + 1]);
                put!(&self.s[self.i..self.i + n].to_vec());
                self.i += n;
            } else if c == b'$' && is_word_char(self.peek_at(1)) {
                // Plain CSS never has this, and passing it through would make
                // the browser drop the rule silently.
                let save = self.i;
                self.i += 1;
                let name = self.word();
                return Err(self.fail(save, format!("use #{{${name}}} to interpolate a variable here")));
            } else if let Some(end) = self.unquoted_url() {
                // Slashes inside an unquoted url() are not a comment.
                put!(&self.s[self.i..self.i + end + 1].to_vec());
                self.i += end + 1;
            } else {
                match c {
                    b'(' | b'[' => depth += 1,
                    b')' | b']' if depth > 0 => depth -= 1,
                    b'{' if braces => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                let n = char_len(c);
                put!(&self.s[self.i..self.i + n].to_vec());
                self.i += n;
            }
        }
        flush!();
        Ok(out)
    }

    /// The expression inside #{...}, cursor on the #.
    fn hole(&mut self) -> Result<Expr> {
        self.enter(self.i)?;
        self.i += 2;
        self.ws();
        let e = self.expr()?;
        self.expect(b'}')?;
        self.leave();
        Ok(e)
    }

    /// If the cursor is at an unquoted url( that runs to a closing
    /// parenthesis without interpolation, the offset of that parenthesis.
    fn unquoted_url(&self) -> Option<usize> {
        let rest = &self.s[self.i..];
        if rest.len() < 5 || !rest[..4].eq_ignore_ascii_case(b"url(") {
            return None;
        }
        let end = find(&rest[4..], b")")?;
        let body = text(&rest[4..4 + end]);
        let body = body.trim();
        if body.is_empty() || body.starts_with('"') || body.starts_with('\'') || body.contains("#{") {
            return None;
        }
        Some(4 + end)
    }

    fn assign(&mut self) -> Result<Stmt> {
        let start = self.i;
        self.i += 1; // $
        let name = self.ident()?;
        self.expect(b':')?;
        self.ws();
        let value = self.expr()?;
        self.ws();
        let mut default = false;
        if self.peek() == b'!' {
            let bang = self.i;
            self.i += 1;
            self.ws();
            match self.word().as_str() {
                "default" => default = true,
                "global" => {
                    return Err(self.fail(bang, "!global is not needed: assignment updates the nearest scope that defines the variable"))
                }
                _ => return Err(self.fail(bang, "expected !default")),
            }
        }
        let span = self.span(start);
        self.end_stmt()?;
        Ok(Stmt::Assign { span, name, value, default })
    }

    fn ident(&mut self) -> Result<String> {
        let start = self.i;
        let w = self.word();
        if w.is_empty() {
            return Err(self.fail(start, "expected a name"));
        }
        Ok(w)
    }

    /// A plain identifier (no interpolation).
    fn word(&mut self) -> String {
        let start = self.i;
        while !self.eof() {
            let c = self.s[self.i];
            if is_word_char(c) {
                self.i += char_len(c);
            } else if c == b'\\' && self.i + 1 < self.s.len() {
                self.i += 1 + char_len(self.s[self.i + 1]);
            } else {
                break;
            }
        }
        self.slice(start, self.i)
    }

    fn at_rule(&mut self) -> Result<Stmt> {
        let start = self.i;
        self.i += 1; // @
        let name = self.ident()?;
        self.ws();
        match name.as_str() {
            "use" => {
                if self.depth > 0 {
                    return Err(self.fail(start, "@use must be at the top level of a file"));
                }
                let q = self.peek();
                if q != b'"' && q != b'\'' {
                    return Err(self.fail(self.i, "@use expects a quoted path"));
                }
                let end = self.skip_string(self.i);
                if end >= self.s.len() || self.s[end] != q {
                    return Err(self.fail(self.i, "unterminated string"));
                }
                let path = self.slice(self.i + 1, end);
                self.i = end + 1;
                let span = self.span(start);
                self.end_stmt()?;
                Ok(Stmt::Use { span, path })
            }
            // CSS's own @function --name and @mixin --name (CSS Functions
            // and Mixins Level 1) are not ours.
            "mixin" | "function" if !self.at("--") => {
                let fname = self.ident()?;
                let params = Rc::new(self.params()?);
                let body = Rc::new(self.block()?);
                let span = self.span(start);
                Ok(if name == "mixin" {
                    Stmt::Mixin { span, name: fname, params, body }
                } else {
                    Stmt::Function { span, name: fname, params, body }
                })
            }
            "include" => {
                let iname = self.ident()?;
                self.ws();
                let args = if self.peek() == b'(' { self.args()? } else { Vec::new() };
                self.ws();
                if self.peek() == b'{' {
                    let body = Some(Rc::new(self.block()?));
                    return Ok(Stmt::Include { span: self.span(start), name: iname, args, body });
                }
                let span = self.span(start);
                self.end_stmt()?;
                Ok(Stmt::Include { span, name: iname, args, body: None })
            }
            "content" => {
                let span = self.span(start);
                self.end_stmt()?;
                Ok(Stmt::Content { span })
            }
            "return" | "error" | "debug" => {
                let value = self.expr()?;
                let span = self.span(start);
                self.end_stmt()?;
                Ok(match name.as_str() {
                    "return" => Stmt::Return { span, value },
                    "error" => Stmt::Log { span, kind: LogKind::Error, value },
                    _ => Stmt::Log { span, kind: LogKind::Debug, value },
                })
            }
            "if" => self.if_rule(start),
            "each" => {
                let mut vars = Vec::new();
                loop {
                    self.ws();
                    if self.peek() != b'$' {
                        return Err(self.fail(self.i, "expected a $variable"));
                    }
                    self.i += 1;
                    vars.push(self.ident()?);
                    self.ws();
                    if self.peek() != b',' {
                        break;
                    }
                    self.i += 1;
                }
                if self.word() != "in" {
                    return Err(self.fail(self.i, "expected 'in'"));
                }
                self.ws();
                let list = self.expr()?;
                let body = self.block()?;
                Ok(Stmt::Each { span: self.span(start), vars, list, body })
            }
            "for" => {
                if self.peek() != b'$' {
                    return Err(self.fail(self.i, "expected a $variable"));
                }
                self.i += 1;
                let var = self.ident()?;
                self.ws();
                if self.word() != "from" {
                    return Err(self.fail(self.i, "expected 'from'"));
                }
                // The bounds are single expressions: a space list would
                // swallow the `through` keyword.
                self.ws();
                let from = self.or_expr()?;
                self.ws();
                let kw = self.word();
                if kw != "to" && kw != "through" {
                    return Err(self.fail(self.i, "expected 'to' or 'through'"));
                }
                self.ws();
                let to = self.or_expr()?;
                let body = self.block()?;
                Ok(Stmt::For { span: self.span(start), var, from, to, inclusive: kw == "through", body })
            }
            "while" => {
                let cond = self.expr()?;
                let body = self.block()?;
                Ok(Stmt::While { span: self.span(start), cond, body })
            }
            _ => {
                let prelude = self.raw("{;", false)?;
                self.ws();
                let body = if self.peek() == b'{' {
                    Some(self.block()?)
                } else {
                    self.end_stmt()?;
                    None
                };
                Ok(Stmt::AtRule { span: self.span(start), name, prelude, body })
            }
        }
    }

    fn if_rule(&mut self, start: usize) -> Result<Stmt> {
        let cond = self.expr()?;
        let then = self.block()?;
        let mut otherwise = Vec::new();
        let save = self.i;
        self.ws();
        if self.at("@else") && !is_word_char(self.peek_at(5)) {
            self.i += 5;
            self.ws();
            if self.at("if") && !is_word_char(self.peek_at(2)) {
                let else_start = self.i;
                self.i += 2;
                self.ws();
                otherwise = vec![self.if_rule(else_start)?];
            } else {
                otherwise = self.block()?;
            }
        } else {
            self.i = save;
        }
        Ok(Stmt::If { span: self.span(start), cond, then, otherwise })
    }

    fn params(&mut self) -> Result<Vec<Param>> {
        let mut out = Vec::new();
        self.ws();
        if self.peek() != b'(' {
            return Ok(out);
        }
        self.i += 1;
        loop {
            self.ws();
            if self.peek() == b')' {
                self.i += 1;
                return Ok(out);
            }
            if self.peek() != b'$' {
                return Err(self.fail(self.i, "expected a $parameter"));
            }
            self.i += 1;
            let name = self.ident()?;
            self.ws();
            let mut prm = Param { name, default: None, rest: false };
            if self.at("...") {
                self.i += 3;
                prm.rest = true;
            } else if self.peek() == b':' {
                self.i += 1;
                self.ws();
                prm.default = Some(self.space_list()?);
            }
            let rest = prm.rest;
            out.push(prm);
            self.ws();
            if self.peek() == b',' {
                self.i += 1;
                continue;
            }
            if rest && self.peek() != b')' {
                return Err(self.fail(self.i, "a rest parameter must be last"));
            }
        }
    }

    /// A call's argument list, cursor on the opening parenthesis.
    fn args(&mut self) -> Result<Vec<Expr>> {
        let mut out = Vec::new();
        self.ws();
        self.enter(self.i)?;
        self.expect(b'(')?;
        loop {
            self.ws();
            if self.peek() == b')' {
                self.i += 1;
                self.leave();
                return Ok(out);
            }
            if self.peek() == b'$' {
                // A Sass habit worth a real message.
                let save = self.i;
                self.i += 1;
                self.word();
                self.ws();
                if self.peek() == b':' {
                    return Err(self.fail(save, "keyword arguments are not supported; pass arguments by position"));
                }
                self.i = save;
            }
            out.push(self.space_list()?);
            self.ws();
            match self.peek() {
                b',' => self.i += 1,
                b')' => {}
                _ => return Err(self.fail(self.i, "expected ',' or ')'")),
            }
        }
    }

    // --- expressions --------------------------------------------------------

    /// A full expression: a comma list of space lists.
    fn expr(&mut self) -> Result<Expr> {
        let start = self.i;
        let first = self.space_list()?;
        self.ws();
        if self.peek() != b',' {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.peek() == b',' {
            self.i += 1;
            self.ws();
            if !self.starts_primary() {
                break; // trailing comma
            }
            items.push(self.space_list()?);
            self.ws();
        }
        Ok(Expr::List { span: self.span(start), items, comma: true })
    }

    fn space_list(&mut self) -> Result<Expr> {
        let start = self.i;
        let first = self.or_expr()?;
        let mut items: Vec<Expr> = Vec::new();
        loop {
            self.ws();
            if !self.starts_primary() {
                break;
            }
            if items.is_empty() {
                items.push(first_placeholder());
            }
            items.push(self.or_expr()?);
        }
        if items.is_empty() {
            return Ok(first);
        }
        items[0] = first;
        Ok(Expr::List { span: self.span(start), items, comma: false })
    }

    /// Whether the next character can begin an operand, which is how a space
    /// list knows where it ends.
    fn starts_primary(&self) -> bool {
        let c = self.peek();
        match c {
            0 => false,
            b'#' => true,
            b'.' => is_digit(self.peek_at(1)),
            b'-' | b'+' => {
                let n = self.peek_at(1);
                is_digit(n) || n == b'.' || n == b'$' || n == b'(' || n == b'-' || is_alpha(n) || n == b'_' || n == b'#' || n == b'\\' || n >= 0x80
            }
            _ => is_alpha(c) || is_digit(c) || matches!(c, b'$' | b'"' | b'\'' | b'(' | b'[' | b'_' | b'\\') || c >= 0x80,
        }
    }

    /// Consumes an operator word if it is exactly there.
    fn keyword(&mut self, kw: &str) -> bool {
        if self.at(kw) && !is_word_char(self.peek_at(kw.len())) {
            self.i += kw.len();
            return true;
        }
        false
    }

    fn or_expr(&mut self) -> Result<Expr> {
        let start = self.i;
        let mut l = self.and_expr()?;
        loop {
            let save = self.i;
            self.ws();
            if !self.keyword("or") {
                self.i = save;
                return Ok(l);
            }
            self.ws();
            let r = self.and_expr()?;
            l = Expr::Binary { span: self.span(start), op: Op::Or, l: Box::new(l), r: Box::new(r) };
        }
    }

    fn and_expr(&mut self) -> Result<Expr> {
        let start = self.i;
        let mut l = self.not_expr()?;
        loop {
            let save = self.i;
            self.ws();
            if !self.keyword("and") {
                self.i = save;
                return Ok(l);
            }
            self.ws();
            let r = self.not_expr()?;
            l = Expr::Binary { span: self.span(start), op: Op::And, l: Box::new(l), r: Box::new(r) };
        }
    }

    fn not_expr(&mut self) -> Result<Expr> {
        let start = self.i;
        if self.keyword("not") {
            self.ws();
            let x = self.not_expr()?;
            return Ok(Expr::Unary { span: self.span(start), op: UnOp::Not, x: Box::new(x) });
        }
        self.eq_expr()
    }

    fn eq_expr(&mut self) -> Result<Expr> {
        let start = self.i;
        let mut l = self.rel_expr()?;
        loop {
            let save = self.i;
            self.ws();
            let op = if self.at("==") {
                Op::Eq
            } else if self.at("!=") {
                Op::Ne
            } else {
                self.i = save;
                return Ok(l);
            };
            self.i += 2;
            self.ws();
            let r = self.rel_expr()?;
            l = Expr::Binary { span: self.span(start), op, l: Box::new(l), r: Box::new(r) };
        }
    }

    fn rel_expr(&mut self) -> Result<Expr> {
        let start = self.i;
        let mut l = self.add_expr()?;
        loop {
            let save = self.i;
            self.ws();
            let (op, n) = if self.at("<=") {
                (Op::Le, 2)
            } else if self.at(">=") {
                (Op::Ge, 2)
            } else if self.peek() == b'<' {
                (Op::Lt, 1)
            } else if self.peek() == b'>' {
                (Op::Gt, 1)
            } else {
                self.i = save;
                return Ok(l);
            };
            self.i += n;
            self.ws();
            let r = self.add_expr()?;
            l = Expr::Binary { span: self.span(start), op, l: Box::new(l), r: Box::new(r) };
        }
    }

    fn add_expr(&mut self) -> Result<Expr> {
        let start = self.i;
        let mut l = self.mul_expr()?;
        loop {
            let save = self.i;
            let before = self.ws();
            let c = self.peek();
            if c != b'+' && c != b'-' {
                self.i = save;
                return Ok(l);
            }
            let after = is_space(self.peek_at(1));
            // `$a -$b` is a space list of $a and -$b; `$a - $b` and `1-2`
            // subtract. An identifier has already swallowed its own dashes.
            if before && !after {
                self.i = save;
                return Ok(l);
            }
            self.i += 1;
            self.ws();
            let r = self.mul_expr()?;
            let op = if c == b'+' { Op::Add } else { Op::Sub };
            l = Expr::Binary { span: self.span(start), op, l: Box::new(l), r: Box::new(r) };
        }
    }

    fn mul_expr(&mut self) -> Result<Expr> {
        let start = self.i;
        let mut l = self.unary()?;
        loop {
            let save = self.i;
            self.ws();
            let op = match self.peek() {
                b'*' => Op::Mul,
                b'/' => Op::Div,
                b'%' => Op::Mod,
                _ => {
                    self.i = save;
                    return Ok(l);
                }
            };
            self.i += 1;
            self.ws();
            let r = self.unary()?;
            l = Expr::Binary { span: self.span(start), op, l: Box::new(l), r: Box::new(r) };
        }
    }

    fn unary(&mut self) -> Result<Expr> {
        let start = self.i;
        let c = self.peek();
        if (c == b'-' || c == b'+') && !is_digit(self.peek_at(1)) && self.peek_at(1) != b'.' {
            let n = self.peek_at(1);
            if matches!(n, b'$' | b'(' | b'-' | b'+') {
                self.i += 1;
                let x = self.unary()?;
                let op = if c == b'-' { UnOp::Neg } else { UnOp::Plus };
                return Ok(Expr::Unary { span: self.span(start), op, x: Box::new(x) });
            }
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr> {
        let start = self.i;
        let c = self.peek();
        if is_digit(c) || c == b'.' || ((c == b'-' || c == b'+') && (is_digit(self.peek_at(1)) || self.peek_at(1) == b'.')) {
            return self.number();
        }
        match c {
            b'$' => {
                self.i += 1;
                let name = self.ident()?;
                return Ok(Expr::Var { span: self.span(start), name });
            }
            b'"' | b'\'' => return self.quoted(),
            b'(' => return self.paren(),
            b'[' => {
                // Grid line names: raw text, so `[full-start]` is what was written.
                self.enter(self.i)?;
                self.i += 1;
                let body = self.raw("]", false)?;
                self.expect(b']')?;
                self.leave();
                let mut parts = vec![Part::Text("[".into())];
                parts.extend(body);
                parts.push(Part::Text("]".into()));
                return Ok(Expr::Str { span: self.span(start), parts, quote: None });
            }
            b'u' | b'U' if self.peek_at(1) == b'+' && is_hex_or_wild(self.peek_at(2)) => return Ok(self.unicode_range()),
            _ => {}
        }
        let w = self.word_interp()?;
        if w.is_empty() {
            return Err(self.fail(start, "expected a value"));
        }
        if let [Part::Text(t)] = w.as_slice() {
            let lit = match t.as_str() {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                "null" => Some(Value::Null),
                _ => None,
            };
            if let Some(val) = lit {
                return Ok(Expr::Lit { span: self.span(start), val });
            }
        }
        if self.peek() == b'(' {
            let name = match w.as_slice() {
                [Part::Text(t)] => t.clone(),
                _ => return Err(self.fail(self.i, "an interpolated name cannot be called")),
            };
            if name.eq_ignore_ascii_case("url") {
                let save = self.i;
                self.i += 1;
                // Only spaces: ws() would take url(//cdn/x.png) for a comment.
                while is_space(self.peek()) {
                    self.i += 1;
                }
                let q = self.peek();
                if q != b'"' && q != b'\'' && q != b')' {
                    // Unquoted url() is raw text: slashes, semicolons and
                    // colons inside it mean nothing to lace, only #{...} does.
                    let mut parts = vec![Part::Text("url(".into())];
                    let mut text: Vec<u8> = Vec::new();
                    while !self.eof() && self.peek() != b')' {
                        if self.at("#{") {
                            parts.push(Part::Text(String::from_utf8_lossy(&text).into_owned()));
                            text.clear();
                            parts.push(Part::Hole(self.hole()?));
                            continue;
                        }
                        text.push(self.s[self.i]);
                        self.i += 1;
                    }
                    let tail = String::from_utf8_lossy(&text).trim().to_string();
                    parts.push(Part::Text(tail + ")"));
                    self.expect(b')')?;
                    return Ok(Expr::Str { span: self.span(start), parts, quote: None });
                }
                self.i = save;
            }
            if self.args_are_css() {
                // `if(style(--wide: true): 100%; else: 50%)` and whatever CSS
                // invents next: colons and semicolons at the top level of an
                // argument list are not lace syntax, so the text is kept raw.
                self.i += 1;
                let body = self.raw(")", false)?;
                self.expect(b')')?;
                return Ok(Expr::Call { span: self.span(start), name, args: Vec::new(), raw: Some(body) });
            }
            let args = self.args()?;
            return Ok(Expr::Call { span: self.span(start), name, args, raw: None });
        }
        Ok(Expr::Str { span: self.span(start), parts: w, quote: None })
    }

    /// Whether the argument list at the cursor holds a `:` or `;` outside
    /// nested parentheses and strings.
    fn args_are_css(&self) -> bool {
        let s = &self.s;
        let mut depth = 0i32;
        let mut i = self.i;
        while i < s.len() {
            match s[i] {
                b'\\' => i += 1,
                b'"' | b'\'' => i = self.skip_string(i),
                b'$' if depth == 1 => {
                    // `$name:` is a keyword-argument attempt, which args()
                    // explains; it is never CSS.
                    let mut j = i + 1;
                    while j < s.len() && is_word_char(s[j]) {
                        j += 1;
                    }
                    while j < s.len() && is_space(s[j]) {
                        j += 1;
                    }
                    if j < s.len() && s[j] == b':' {
                        return false;
                    }
                }
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return false;
                    }
                }
                b':' | b';' if depth == 1 => return true,
                _ => {}
            }
            i += 1;
        }
        false
    }

    /// U+0025-00FF or U+4?? as one word: `+` and `?` belong to no other token.
    fn unicode_range(&mut self) -> Expr {
        let start = self.i;
        self.i += 2;
        let mut n = 0;
        while n < 6 && is_hex_or_wild(self.peek()) {
            self.i += 1;
            n += 1;
        }
        if self.peek() == b'-' && is_hex_or_wild(self.peek_at(1)) {
            self.i += 1;
            let mut n = 0;
            while n < 6 && is_hex_or_wild(self.peek()) {
                self.i += 1;
                n += 1;
            }
        }
        let t = self.slice(start, self.i);
        Expr::Str { span: self.span(start), parts: vec![Part::Text(t)], quote: None }
    }

    fn number(&mut self) -> Result<Expr> {
        let start = self.i;
        if matches!(self.peek(), b'-' | b'+') {
            self.i += 1;
        }
        while is_digit(self.peek()) {
            self.i += 1;
        }
        if self.peek() == b'.' && is_digit(self.peek_at(1)) {
            self.i += 1;
            while is_digit(self.peek()) {
                self.i += 1;
            }
        }
        if matches!(self.peek(), b'e' | b'E') {
            let n = self.peek_at(1);
            if is_digit(n) || (matches!(n, b'-' | b'+') && is_digit(self.peek_at(2))) {
                self.i += 2;
                while is_digit(self.peek()) {
                    self.i += 1;
                }
            }
        }
        let t = self.slice(start, self.i);
        let val: f64 = match t.trim_start_matches('+').parse() {
            Ok(v) => v,
            Err(_) => return Err(self.fail(start, format!("bad number {t:?}"))),
        };
        let unit_start = self.i;
        if self.peek() == b'%' {
            self.i += 1;
        } else {
            while is_alpha(self.peek()) {
                self.i += 1;
            }
        }
        let unit = self.slice(unit_start, self.i);
        Ok(Expr::Num { span: self.span(start), val, unit })
    }

    fn quoted(&mut self) -> Result<Expr> {
        let start = self.i;
        let q = self.s[self.i];
        self.i += 1;
        let mut parts: Interp = Vec::new();
        let mut text: Vec<u8> = Vec::new();
        loop {
            if self.eof() || self.peek() == b'\n' {
                return Err(self.fail(start, "unterminated string"));
            }
            let c = self.s[self.i];
            if c == q {
                self.i += 1;
                if !text.is_empty() || parts.is_empty() {
                    parts.push(Part::Text(String::from_utf8_lossy(&text).into_owned()));
                }
                return Ok(Expr::Str { span: self.span(start), parts, quote: Some(q as char) });
            } else if c == b'\\' {
                text.push(c);
                self.i += 1;
                if !self.eof() {
                    let n = char_len(self.s[self.i]);
                    text.extend_from_slice(&self.s[self.i..self.i + n]);
                    self.i += n;
                }
            } else if c == b'#' && self.peek_at(1) == b'{' {
                if !text.is_empty() {
                    parts.push(Part::Text(String::from_utf8_lossy(&text).into_owned()));
                    text.clear();
                }
                parts.push(Part::Hole(self.hole()?));
            } else {
                text.push(c);
                self.i += 1;
            }
        }
    }

    /// An identifier that may contain #{...} holes; `#` may also start a hex
    /// colour word.
    fn word_interp(&mut self) -> Result<Interp> {
        let mut parts: Interp = Vec::new();
        let mut text: Vec<u8> = Vec::new();
        while !self.eof() {
            let c = self.s[self.i];
            if c == b'#' && self.peek_at(1) == b'{' {
                if !text.is_empty() {
                    parts.push(Part::Text(String::from_utf8_lossy(&text).into_owned()));
                    text.clear();
                }
                parts.push(Part::Hole(self.hole()?));
            } else if is_word_char(c) || (c == b'#' && text.is_empty() && parts.is_empty()) {
                let n = char_len(c);
                text.extend_from_slice(&self.s[self.i..self.i + n]);
                self.i += n;
            } else if c == b'\\' && self.i + 1 < self.s.len() {
                let n = 1 + char_len(self.s[self.i + 1]);
                text.extend_from_slice(&self.s[self.i..self.i + n]);
                self.i += n;
            } else {
                break;
            }
        }
        if !text.is_empty() {
            parts.push(Part::Text(String::from_utf8_lossy(&text).into_owned()));
        }
        Ok(parts)
    }

    /// `(...)`: an empty list, a map, a comma list, or a grouped expression,
    /// decided by what follows the first element.
    fn paren(&mut self) -> Result<Expr> {
        let start = self.i;
        self.enter(start)?;
        self.i += 1;
        self.ws();
        if self.peek() == b')' {
            self.i += 1;
            self.leave();
            return Ok(Expr::List { span: self.span(start), items: Vec::new(), comma: false });
        }
        let first = self.space_list()?;
        self.ws();
        let result = match self.peek() {
            b':' => {
                let mut pairs = Vec::new();
                let mut key = first;
                loop {
                    self.expect(b':')?;
                    self.ws();
                    let val = self.space_list()?;
                    pairs.push((key, val));
                    self.ws();
                    if self.peek() != b',' {
                        break;
                    }
                    self.i += 1;
                    self.ws();
                    if self.peek() == b')' {
                        break;
                    }
                    key = self.space_list()?;
                    self.ws();
                }
                self.expect(b')')?;
                Expr::Map { span: self.span(start), pairs }
            }
            b',' => {
                let mut items = vec![first];
                while self.peek() == b',' {
                    self.i += 1;
                    self.ws();
                    if self.peek() == b')' {
                        break;
                    }
                    items.push(self.space_list()?);
                    self.ws();
                }
                self.expect(b')')?;
                Expr::List { span: self.span(start), items, comma: true }
            }
            _ => {
                self.expect(b')')?;
                Expr::Paren { span: self.span(start), x: Box::new(first) }
            }
        };
        self.leave();
        Ok(result)
    }
}

/// A placeholder swapped for the real first item once a space list turns out
/// to have more than one.
fn first_placeholder() -> Expr {
    Expr::Lit { span: Span { src: Source::new("", ""), a: 0, b: 0 }, val: Value::Null }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}
