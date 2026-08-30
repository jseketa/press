//! Runtime values, their arithmetic and their serialisation.

use std::rc::Rc;

/// Six kinds and no colour: `#fff` is a Str, and colour arithmetic is what
/// color-mix() is for.
#[derive(Clone, Debug)]
pub enum Value {
    /// A number remembers the text it was written with, so an untouched
    /// literal reaches the output as written; arithmetic clears it.
    Num { v: f64, unit: Rc<str>, text: Option<Rc<str>> },
    /// Quoted (`quote` is the quote character) or unquoted. Identifiers,
    /// keywords and CSS functions passed through are all unquoted.
    Str { s: Rc<str>, quote: Option<char> },
    Bool(bool),
    Null,
    List(Rc<List>),
    /// Insertion-ordered; lookups are linear because maps are the size of a
    /// design token table, not a database.
    Map(Rc<Vec<(Value, Value)>>),
}

#[derive(Debug)]
pub struct List {
    pub items: Vec<Value>,
    pub comma: bool,
}

/// Limits that turn runaway programs into errors instead of memory
/// exhaustion.
pub const MAX_STRING_LEN: usize = 1 << 20;
pub const MAX_LIST_LEN: usize = 1_000_000;

impl Value {
    pub fn num(v: f64, unit: &str) -> Value {
        Value::Num { v, unit: unit.into(), text: None }
    }

    pub fn unquoted(s: impl Into<Rc<str>>) -> Value {
        Value::Str { s: s.into(), quote: None }
    }

    pub fn quoted(s: impl Into<Rc<str>>) -> Value {
        Value::Str { s: s.into(), quote: Some('"') }
    }

    pub fn list(items: Vec<Value>, comma: bool) -> Value {
        Value::List(Rc::new(List { items, comma }))
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Value::Num { .. } => "number",
            Value::Str { .. } => "string",
            Value::Bool(_) => "bool",
            Value::Null => "null",
            Value::List(_) => "list",
            Value::Map(_) => "map",
        }
    }

    pub fn truthy(&self) -> bool {
        !matches!(self, Value::Null | Value::Bool(false))
    }

    pub fn is_string(&self) -> bool {
        matches!(self, Value::Str { .. })
    }

    /// Any value viewed as a list: a list is itself, a map is its pairs as
    /// `key value` items, anything else is a one-item space list.
    pub fn as_list(&self) -> Rc<List> {
        match self {
            Value::List(l) => l.clone(),
            Value::Map(m) => Rc::new(List {
                items: m.iter().map(|(k, v)| Value::list(vec![k.clone(), v.clone()], false)).collect(),
                comma: true,
            }),
            other => Rc::new(List { items: vec![other.clone()], comma: false }),
        }
    }

    /// A map, or the empty list, which is how an empty map is written.
    pub fn as_map(&self) -> Option<Rc<Vec<(Value, Value)>>> {
        match self {
            Value::Map(m) => Some(m.clone()),
            Value::List(l) if l.items.is_empty() => Some(Rc::new(Vec::new())),
            _ => None,
        }
    }

    /// Null, or a list with nothing to print: an optional declaration value.
    pub fn is_null(&self) -> bool {
        match self {
            Value::Null => true,
            Value::List(l) => l.items.iter().all(Value::is_null),
            _ => false,
        }
    }
}

/// Numeric equality at the output precision: two numbers that print the
/// same are the same, so a loop stepping by 0.1 can reach 1.
pub fn nearly(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-10
}

pub fn equal(a: &Value, b: &Value) -> bool {
    // A one-item list is its item.
    if let Value::List(l) = a {
        if l.items.len() == 1 && !matches!(b, Value::List(_)) {
            return equal(&l.items[0], b);
        }
    }
    if let Value::List(l) = b {
        if l.items.len() == 1 && !matches!(a, Value::List(_)) {
            return equal(a, &l.items[0]);
        }
    }
    match (a, b) {
        (Value::Num { v: x, unit: ux, .. }, Value::Num { v: y, unit: uy, .. }) => nearly(*x, *y) && ux == uy,
        (Value::Str { s: x, .. }, Value::Str { s: y, .. }) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Null, Value::Null) => true,
        (Value::List(x), Value::List(y)) => {
            x.items.len() == y.items.len()
                && (x.items.len() < 2 || x.comma == y.comma)
                && x.items.iter().zip(&y.items).all(|(p, q)| equal(p, q))
        }
        (Value::Map(x), Value::Map(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| map_get(y, k).map_or(false, |w| equal(v, w)))
        }
        _ => false,
    }
}

pub fn map_get<'a>(m: &'a [(Value, Value)], k: &Value) -> Option<&'a Value> {
    m.iter().find(|(mk, _)| equal(mk, k)).map(|(_, v)| v)
}

/// A copy of the map with k bound to v, keeping the position of an existing key.
pub fn map_set(m: &[(Value, Value)], k: Value, v: Value) -> std::result::Result<Value, String> {
    let mut n: Vec<(Value, Value)> = m.to_vec();
    if let Some(slot) = n.iter_mut().find(|(mk, _)| equal(mk, &k)) {
        slot.1 = v;
        return Ok(Value::Map(Rc::new(n)));
    }
    if n.len() >= MAX_LIST_LEN {
        return Err("map too large".into());
    }
    n.push((k, v));
    Ok(Value::Map(Rc::new(n)))
}

// --- arithmetic -------------------------------------------------------------

/// The unit of a binary operation, or why there is none.
fn unit_of(op: &str, a: (f64, &str), b: (f64, &str)) -> std::result::Result<String, String> {
    let (ua, ub) = (a.1, b.1);
    if ua == ub {
        return Ok(if op == "/" { String::new() } else { ua.to_string() });
    }
    if ua.is_empty() {
        return Ok(ub.to_string());
    }
    if ub.is_empty() {
        return Ok(ua.to_string());
    }
    Err(format!("{}{} {} {}{}: incompatible units", format_number(a.0), ua, op, format_number(b.0), ub))
}

/// Applies an arithmetic or ordering operator to two numbers, which are
/// passed with their spelling so that error messages read as written.
pub fn num_op(op: &str, l: &Value, r: &Value) -> std::result::Result<Value, String> {
    let (Value::Num { v: x, unit: ua, text: ta }, Value::Num { v: y, unit: ub, text: tb }) = (l, r) else {
        unreachable!()
    };
    let (x, y) = (*x, *y);
    let (a, b) = ((x, &**ua), (y, &**ub));
    let unit = unit_of(op, a, b).map_err(|_| {
        format!("{} {} {}: incompatible units", css_number(x, ua, ta), op, css_number(y, ub, tb))
    })?;
    let text = |v: f64, u: &str| {
        if v == x && u == &**ua {
            css_number(x, ua, ta)
        } else {
            css_number(y, ub, tb)
        }
    };
    let r = match op {
        "+" => x + y,
        "-" => x - y,
        "*" => {
            if !a.1.is_empty() && !b.1.is_empty() {
                return Err(format!("{} * {}: no compound units", text(x, a.1), text(y, b.1)));
            }
            x * y
        }
        "/" | "%" => {
            if y == 0.0 {
                return Err(format!("{} {} {}: division by zero", text(x, a.1), op, text(y, b.1)));
            }
            if op == "/" {
                x / y
            } else {
                x % y
            }
        }
        "<" => return Ok(Value::Bool(x < y && !nearly(x, y))),
        ">" => return Ok(Value::Bool(x > y && !nearly(x, y))),
        "<=" => return Ok(Value::Bool(x <= y || nearly(x, y))),
        ">=" => return Ok(Value::Bool(x >= y || nearly(x, y))),
        _ => unreachable!(),
    };
    if !r.is_finite() {
        return Err(format!("{} {} {}: result is not a number", text(x, a.1), op, text(y, b.1)));
    }
    Ok(Value::num(r, &unit))
}

// --- serialisation ----------------------------------------------------------

/// At most ten decimals, no trailing zeros, no -0.
pub fn format_number(f: f64) -> String {
    let mut s = format!("{f:.10}");
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if s == "-0" || s.is_empty() {
        return "0".into();
    }
    s
}

pub fn css_number(v: f64, unit: &str, text: &Option<Rc<str>>) -> String {
    match text {
        Some(t) => t.to_string(),
        None => format!("{}{}", format_number(v), unit),
    }
}

pub fn css_string(s: &str, quote: Option<char>) -> String {
    match quote {
        Some(q) => format!("{q}{s}{q}"),
        None => s.to_string(),
    }
}

/// A value as CSS text. Values with no CSS form are an error; the caller
/// decides whether null means "omit" instead. Null items inside a list are
/// left out, so `transition: $a, $b` works when $b is unset.
pub fn css(v: &Value) -> std::result::Result<String, String> {
    match v {
        Value::Num { v, unit, text } => Ok(css_number(*v, unit, text)),
        Value::Str { s, quote } => Ok(css_string(s, *quote)),
        // `inherits: $flag` in @property is the one place a bool is CSS.
        Value::Bool(b) => Ok(b.to_string()),
        Value::List(l) => {
            if l.items.is_empty() {
                return Err("() is not a CSS value".into());
            }
            let mut out = String::new();
            for it in &l.items {
                if it.is_null() {
                    continue;
                }
                let s = css(it)?;
                if !out.is_empty() {
                    out.push_str(if l.comma { ", " } else { " " });
                }
                out.push_str(&s);
            }
            Ok(out)
        }
        other => Err(format!("{} is not a CSS value", inspect(other))),
    }
}

/// A value the way @debug shows it: unambiguous, for humans.
pub fn inspect(v: &Value) -> String {
    match v {
        Value::Num { v, unit, text } => css_number(*v, unit, text),
        Value::Str { s, quote } => css_string(s, *quote),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".into(),
        Value::List(l) => {
            let sep = if l.comma { ", " } else { " " };
            let s = l.items.iter().map(inspect).collect::<Vec<_>>().join(sep);
            match l.items.len() {
                1 if l.comma => format!("({s},)"),
                0 | 1 => format!("({s})"),
                _ => s,
            }
        }
        Value::Map(m) => {
            let s = m.iter().map(|(k, v)| format!("{}: {}", inspect(k), inspect(v))).collect::<Vec<_>>().join(", ");
            format!("({s})")
        }
    }
}

/// A value for interpolation: like css, but a quoted string loses its
/// quotes (only at the top level: items in a list keep theirs), and null or
/// an empty list is nothing.
pub fn str_of(v: &Value) -> std::result::Result<String, String> {
    match v {
        Value::Str { s, .. } => Ok(s.to_string()),
        Value::Null => Ok(String::new()),
        Value::List(l) if l.items.is_empty() => Ok(String::new()),
        other => css(other),
    }
}
