//! Builtins are the operations the language cannot express about its own
//! values (append, map-set, type-of, unit, inspect) plus the three it could
//! express but not in constant time (length, nth, map-get). Everything else
//! is written in lace in prelude.scss.

use std::rc::Rc;

use crate::eval::{Callable, Env};
use crate::parse::parse_params;
use crate::value::*;

type R = std::result::Result<Value, String>;

const BUILTINS: &[(&str, &str, fn(&[Value]) -> R)] = &[
    ("length", "($list)", |a| Ok(Value::num(a[0].as_list().items.len() as f64, ""))),
    ("nth", "($list, $n)", |a| {
        let items = &a[0].as_list().items;
        let i = match &a[1] {
            Value::Num { v, unit, .. } if unit.is_empty() && *v == v.trunc() => *v as i64,
            other => return Err(format!("$n must be a unitless integer, got {}", inspect(other))),
        };
        let idx = if i < 0 { i + items.len() as i64 + 1 } else { i };
        if idx < 1 || idx as usize > items.len() {
            return Err(format!("index {} is out of range for {} items", inspect(&a[1]), items.len()));
        }
        Ok(items[idx as usize - 1].clone())
    }),
    ("append", "($list, $value, $separator: auto)", |a| {
        let l = a[0].as_list();
        let comma = match &a[2] {
            Value::Str { s, .. } if &**s == "auto" => l.comma,
            Value::Str { s, .. } if &**s == "comma" => true,
            Value::Str { s, .. } if &**s == "space" => false,
            other => return Err(format!("$separator must be auto, comma or space, got {}", inspect(other))),
        };
        if l.items.len() >= MAX_LIST_LEN {
            return Err("list too large".into());
        }
        let mut items = l.items.clone();
        items.push(a[1].clone());
        Ok(Value::list(items, comma))
    }),
    ("map-get", "($map, $key)", |a| {
        let m = a[0].as_map().ok_or_else(|| format!("expected a map, got {}", inspect(&a[0])))?;
        Ok(map_get(&m, &a[1]).cloned().unwrap_or(Value::Null))
    }),
    ("map-set", "($map, $key, $value)", |a| {
        let m = a[0].as_map().ok_or_else(|| format!("expected a map, got {}", inspect(&a[0])))?;
        map_set(&m, a[1].clone(), a[2].clone())
    }),
    ("type-of", "($value)", |a| Ok(Value::unquoted(a[0].kind()))),
    ("unit", "($number)", |a| match &a[0] {
        Value::Num { unit, .. } => Ok(Value::quoted(unit.clone())),
        other => Err(format!("$number must be a number, got {}", inspect(other))),
    }),
    ("inspect", "($value)", |a| Ok(Value::unquoted(inspect(&a[0])))),
];

/// Defines the builtins in the global scope, parsing each signature with the
/// ordinary parser so binding works exactly as for a user-defined function.
pub fn install_builtins(global: &Env) {
    for (name, sig, f) in BUILTINS {
        let c = Callable {
            kind: "function",
            name: name.to_string(),
            params: Rc::new(parse_params(sig)),
            body: Rc::new(Vec::new()),
            env: global.clone(),
            builtin: Some(*f),
            locked: true,
            content: false,
        };
        global.borrow_mut().funcs.insert(name.to_string(), Rc::new(c));
    }
}
