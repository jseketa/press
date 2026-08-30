//! A tree-walking evaluator. Statements write into the current output block;
//! expressions produce values. Errors carry the location of the statement or
//! expression being evaluated and, through calls, one frame per include or
//! function call.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;

use crate::ast::*;
use crate::builtins::install_builtins;
use crate::parse::{is_space, parse};
use crate::source::{Error, Result, Source, Span};
use crate::value::*;

pub const DEFAULT_BUDGET: i64 = 10_000_000;
const MAX_DEPTH: usize = 1000;

/// Reads the file behind a @use path, already resolved relative to the
/// file that contains the @use, with forward slashes.
pub type Loader = Box<dyn Fn(&str) -> std::result::Result<String, String> + Send>;

/// Where @debug lines go.
pub type Log = Box<dyn Write + Send>;

/// A lexical scope. Variables, mixins and functions are separate namespaces,
/// so `$grid`, `@mixin grid` and `@function grid` can coexist.
pub struct Scope {
    vars: HashMap<String, Value>,
    mixins: HashMap<String, Rc<Callable>>,
    pub(crate) funcs: HashMap<String, Rc<Callable>>,
    parent: Option<Env>,
}

pub type Env = Rc<RefCell<Scope>>;

pub fn new_env(parent: Option<&Env>) -> Env {
    Rc::new(RefCell::new(Scope {
        vars: HashMap::new(),
        mixins: HashMap::new(),
        funcs: HashMap::new(),
        parent: parent.cloned(),
    }))
}

fn lookup(env: &Env, name: &str) -> Option<Value> {
    let mut cur = Some(env.clone());
    while let Some(e) = cur {
        let s = e.borrow();
        if let Some(v) = s.vars.get(name) {
            return Some(v.clone());
        }
        cur = s.parent.clone();
    }
    None
}

/// Updates the nearest scope that already defines name, else defines it
/// here. This is what lets a loop body count up an outer counter without a
/// `!global` escape hatch.
fn assign(env: &Env, name: &str, v: Value) {
    let mut cur = Some(env.clone());
    while let Some(e) = cur {
        let mut s = e.borrow_mut();
        if s.vars.contains_key(name) {
            s.vars.insert(name.to_string(), v);
            return;
        }
        cur = s.parent.clone();
    }
    env.borrow_mut().vars.insert(name.to_string(), v);
}

fn define(env: &Env, name: &str, v: Value) {
    env.borrow_mut().vars.insert(name.to_string(), v);
}

fn find_mixin(env: &Env, name: &str) -> Option<Rc<Callable>> {
    let mut cur = Some(env.clone());
    while let Some(e) = cur {
        let s = e.borrow();
        if let Some(c) = s.mixins.get(name) {
            return Some(c.clone());
        }
        cur = s.parent.clone();
    }
    None
}

pub fn find_function(env: &Env, name: &str) -> Option<Rc<Callable>> {
    let mut cur = Some(env.clone());
    while let Some(e) = cur {
        let s = e.borrow();
        if let Some(c) = s.funcs.get(name) {
            return Some(c.clone());
        }
        cur = s.parent.clone();
    }
    None
}

pub type BuiltinFn = fn(&[Value]) -> std::result::Result<Value, String>;

/// A mixin, a function, or a builtin: parameters, a body, and the scope it
/// closes over.
pub struct Callable {
    pub kind: &'static str,
    pub name: String,
    pub params: Rc<Vec<Param>>,
    pub body: Body,
    pub env: Env,
    pub builtin: Option<BuiltinFn>,
    pub locked: bool, // prelude and builtins cannot be redefined
    pub content: bool, // a mixin whose body reaches @content
}

/// The block passed to an @include, remembered so @content can run it in
/// the scope of the include.
struct ContentBlock {
    body: Body,
    env: Env,
}

/// The output is a tree of blocks whose leaves are declaration and comment
/// lines, so that rules which end up empty can be left out.
struct Block {
    head: String,
    kids: Vec<Node>,
    keep: bool, // emit even when empty: the author wrote `@layer x {}`
}

enum Node {
    Line(String),
    Block(Block),
}

impl Block {
    fn empty(&self) -> bool {
        !self.keep && self.kids.iter().all(|k| matches!(k, Node::Block(b) if b.empty()))
    }

    fn render(&self, w: &mut String, indent: &str) {
        for k in &self.kids {
            match k {
                Node::Line(l) => {
                    w.push_str(indent);
                    w.push_str(l);
                    w.push('\n');
                }
                Node::Block(b) if !b.empty() => {
                    w.push_str(indent);
                    w.push_str(&b.head);
                    if b.kids.is_empty() {
                        w.push_str(" {}\n");
                        continue;
                    }
                    w.push_str(" {\n");
                    b.render(w, &format!("{indent}  "));
                    w.push_str(indent);
                    w.push_str("}\n");
                }
                Node::Block(_) => {}
            }
        }
    }
}

pub struct Evaluator {
    load: Loader,
    log: Log,
    budget: i64,
    pub global: Env,
    out: Vec<Block>, // the stack of open output blocks; empty inside a function
    content: Vec<Option<Rc<ContentBlock>>>,
    file: String, // path of the file being executed, for @use resolution
    loaded: Vec<String>,
    loading: Vec<String>,
    steps: i64,
    depth: usize,
    in_func: usize,
    in_mixin: usize,
    ret: Option<Value>,
}

/// The signal a statement returns: keep going, or a @return happened.
type Flow = Result<bool>;

impl Evaluator {
    pub fn new(load: Loader, log: Log, budget: i64) -> Evaluator {
        let global = new_env(None);
        install_builtins(&global);
        Evaluator {
            load,
            log,
            budget,
            global,
            out: vec![Block { head: String::new(), kids: Vec::new(), keep: true }],
            content: Vec::new(),
            file: String::new(),
            loaded: Vec::new(),
            loading: Vec::new(),
            steps: 0,
            depth: 0,
            in_func: 0,
            in_mixin: 0,
            ret: None,
        }
    }

    /// Runs a file's statements in the global scope.
    pub fn run(&mut self, stmts: &[Stmt], file: &str) -> Result<()> {
        self.file = file.to_string();
        let global = self.global.clone();
        self.exec(stmts, &global)?;
        Ok(())
    }

    /// Locks every function defined so far against redefinition.
    pub fn lock_functions(&mut self) {
        let mut g = self.global.borrow_mut();
        let names: Vec<String> = g.funcs.keys().cloned().collect();
        for name in names {
            let c = g.funcs.remove(&name).unwrap();
            let locked = Callable { locked: true, ..clone_callable(&c) };
            g.funcs.insert(name, Rc::new(locked));
        }
    }

    pub fn reset_steps(&mut self) {
        self.steps = 0;
    }

    pub fn render(&self) -> String {
        let mut w = String::new();
        let mut first = true;
        for k in &self.out[0].kids {
            if let Node::Block(b) = k {
                if b.empty() {
                    continue;
                }
            }
            if !first {
                w.push('\n');
            }
            first = false;
            let one = Block { head: String::new(), kids: Vec::new(), keep: true };
            let _ = one;
            match k {
                Node::Line(l) => {
                    w.push_str(l);
                    w.push('\n');
                }
                Node::Block(b) => {
                    w.push_str(&b.head);
                    if b.kids.is_empty() {
                        w.push_str(" {}\n");
                    } else {
                        w.push_str(" {\n");
                        b.render(&mut w, "  ");
                        w.push_str("}\n");
                    }
                }
            }
        }
        w
    }

    fn step(&mut self, at: &Span) -> Result<()> {
        self.steps += 1;
        if self.budget > 0 && self.steps > self.budget {
            return Err(at.error("evaluation budget exceeded (infinite loop?)"));
        }
        Ok(())
    }

    // --- statements ---------------------------------------------------------

    /// Runs statements in a scope and reports whether a @return happened.
    fn exec(&mut self, stmts: &[Stmt], env: &Env) -> Flow {
        for s in stmts {
            if self.stmt(s, env)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn emit(&mut self, at: &Span, line: String) -> Result<()> {
        match self.out.last_mut() {
            Some(b) => {
                b.kids.push(Node::Line(line));
                Ok(())
            }
            None => Err(at.error("CSS is not allowed inside a function")),
        }
    }

    /// Runs body inside a new output block with the given head.
    fn enter(&mut self, at: &Span, head: String, body: &[Stmt], env: &Env) -> Result<()> {
        if self.out.is_empty() {
            return Err(at.error("CSS is not allowed inside a function"));
        }
        self.out.push(Block { head, kids: Vec::new(), keep: false });
        let r = self.exec(body, env);
        let b = self.out.pop().unwrap();
        r?;
        self.out.last_mut().unwrap().kids.push(Node::Block(b));
        Ok(())
    }

    fn stmt(&mut self, s: &Stmt, env: &Env) -> Flow {
        self.step(s.span())?;
        match s {
            Stmt::Decl { span, name, value, raw, important } => {
                if self.out.len() == 1 {
                    return Err(span.error("a declaration must be inside a rule"));
                }
                let name = self.interp(name, env)?;
                let mut text = match (raw, value) {
                    (Some(raw), _) => self.interp(raw, env)?,
                    (None, Some(value)) => {
                        let v = self.eval(value, env, true)?;
                        if v.is_null() {
                            return Ok(false);
                        }
                        self.css_text(value.span(), &v)?
                    }
                    (None, None) => unreachable!(),
                };
                if !important.is_empty() {
                    text.push(' ');
                    text.push_str(important);
                }
                self.emit(span, format!("{name}: {text};"))?;
            }
            Stmt::Rule { span, selector, body } => {
                let head = self.interp(selector, env)?;
                self.enter(span, head, body, &new_env(Some(env)))?;
            }
            Stmt::AtRule { span, name, prelude, body } => {
                let mut head = format!("@{name}");
                let prelude = self.interp(prelude, env)?;
                if !prelude.is_empty() {
                    head.push(' ');
                    head.push_str(&prelude);
                }
                match body {
                    None => self.emit(span, head + ";")?,
                    Some(body) if body.is_empty() => {
                        // `@layer base {}` declares the layer's place in the
                        // cascade: an empty at-rule block written by the
                        // author is kept.
                        match self.out.last_mut() {
                            Some(b) => b.kids.push(Node::Block(Block { head, kids: Vec::new(), keep: true })),
                            None => return Err(span.error("CSS is not allowed inside a function")),
                        }
                    }
                    Some(body) => self.enter(span, head, body, &new_env(Some(env)))?,
                }
            }
            Stmt::Comment { text, .. } => {
                if let Some(b) = self.out.last_mut() {
                    b.kids.push(Node::Line(text.clone()));
                }
            }
            Stmt::Use { span, path } => self.use_file(span, path)?,
            Stmt::Assign { name, value, default, .. } => {
                if *default {
                    if let Some(v) = lookup(env, name) {
                        if !v.is_null() {
                            return Ok(false);
                        }
                    }
                }
                let v = self.eval(value, env, false)?;
                assign(env, name, v);
            }
            Stmt::Mixin { span, name, params, body } => {
                if env.borrow().mixins.contains_key(name) {
                    return Err(span.error(format!("mixin {name} is already defined in this scope")));
                }
                let c = Callable {
                    kind: "mixin",
                    name: name.clone(),
                    params: params.clone(),
                    body: body.clone(),
                    env: env.clone(),
                    builtin: None,
                    locked: false,
                    content: uses_content(body),
                };
                env.borrow_mut().mixins.insert(name.clone(), Rc::new(c));
            }
            Stmt::Function { span, name, params, body } => {
                if let Some(c) = find_function(&self.global, name) {
                    if c.locked {
                        return Err(span.error(format!("{name} is a built-in function and cannot be redefined")));
                    }
                }
                if env.borrow().funcs.contains_key(name) {
                    return Err(span.error(format!("function {name} is already defined in this scope")));
                }
                let c = Callable {
                    kind: "function",
                    name: name.clone(),
                    params: params.clone(),
                    body: body.clone(),
                    env: env.clone(),
                    builtin: None,
                    locked: false,
                    content: false,
                };
                env.borrow_mut().funcs.insert(name.clone(), Rc::new(c));
            }
            Stmt::Include { span, name, args, body } => {
                let Some(c) = find_mixin(env, name) else {
                    return Err(span.error(format!("unknown mixin {name:?}")));
                };
                if self.out.is_empty() {
                    return Err(span.error("CSS is not allowed inside a function"));
                }
                if body.is_some() && !c.content {
                    return Err(span.error(format!("mixin {name} does not accept a block (it has no @content)")));
                }
                let scope = self.bind(&c, args, span, env)?;
                let cb = body.as_ref().map(|b| Rc::new(ContentBlock { body: b.clone(), env: env.clone() }));
                self.content.push(cb);
                self.in_mixin += 1;
                let r = self.call(&c, span, |ev| ev.exec(&c.body, &scope).map(|_| ()));
                self.in_mixin -= 1;
                self.content.pop();
                r?;
            }
            Stmt::Content { span } => {
                if self.in_mixin == 0 || self.in_func > 0 {
                    return Err(span.error("@content is only allowed inside a mixin"));
                }
                if let Some(Some(cb)) = self.content.pop() {
                    // The block runs with the outer include's content visible.
                    let r = self.exec(&cb.body, &new_env(Some(&cb.env)));
                    self.content.push(Some(cb));
                    r?;
                } else {
                    self.content.push(None);
                }
            }
            Stmt::Return { span, value } => {
                if self.in_func == 0 {
                    return Err(span.error("@return is only allowed inside a function"));
                }
                self.ret = Some(self.eval(value, env, false)?);
                return Ok(true);
            }
            Stmt::If { cond, then, otherwise, .. } => {
                let branch = if self.eval(cond, env, false)?.truthy() { then } else { otherwise };
                return self.exec(branch, &new_env(Some(env)));
            }
            Stmt::Each { vars, list, body, .. } => {
                let list = self.eval(list, env, false)?.as_list();
                for item in &list.items {
                    let scope = new_env(Some(env));
                    if vars.len() == 1 {
                        define(&scope, &vars[0], item.clone());
                    } else {
                        let parts = item.as_list();
                        for (i, name) in vars.iter().enumerate() {
                            define(&scope, name, parts.items.get(i).cloned().unwrap_or(Value::Null));
                        }
                    }
                    if self.exec(body, &scope)? {
                        return Ok(true);
                    }
                }
            }
            Stmt::For { span, var, from, to, inclusive, body } => {
                let (fv, fu) = self.integer(from, env)?;
                let (tv, tu) = self.integer(to, env)?;
                let unit = if fu == tu || tu.is_empty() {
                    fu
                } else if fu.is_empty() {
                    tu
                } else {
                    return Err(span.error(format!("@for bounds {}{} and {}{}: incompatible units", format_number(fv), fu, format_number(tv), tu)));
                };
                let last = if *inclusive { tv } else { tv - 1.0 };
                // Counting up only: `from 1 through length($empty)` runs zero
                // times instead of visiting 1 and 0.
                let mut k = 0.0;
                while fv + k <= last {
                    self.step(span)?;
                    let scope = new_env(Some(env));
                    define(&scope, var, Value::num(fv + k, &unit));
                    if self.exec(body, &scope)? {
                        return Ok(true);
                    }
                    k += 1.0;
                }
            }
            Stmt::While { span, cond, body } => {
                while self.eval(cond, env, false)?.truthy() {
                    self.step(span)?;
                    if self.exec(body, &new_env(Some(env)))? {
                        return Ok(true);
                    }
                }
            }
            Stmt::Log { span, kind, value } => {
                let v = self.eval(value, env, false)?;
                let msg = match &v {
                    Value::Str { s, .. } => s.to_string(),
                    other => inspect(other),
                };
                if *kind == LogKind::Error {
                    return Err(span.error(msg));
                }
                let (line, col) = span.src.pos(span.a);
                let _ = writeln!(self.log, "{}:{}:{}: debug: {}", span.src.name, line, col, msg);
            }
        }
        Ok(false)
    }

    fn integer(&mut self, x: &Expr, env: &Env) -> Result<(f64, String)> {
        let v = self.eval(x, env, false)?;
        match v {
            Value::Num { v, ref unit, .. } if v == v.trunc() => Ok((v, unit.to_string())),
            other => Err(x.span().error(format!("@for bound must be an integer, got {}", inspect(&other)))),
        }
    }

    /// Compiles another file into the current output and the global scope.
    fn use_file(&mut self, span: &Span, path: &str) -> Result<()> {
        let key = resolve(&self.file, path);
        if self.loading.contains(&key) {
            return Err(span.error(format!("@use cycle through {path:?}")));
        }
        if self.loaded.contains(&key) {
            return Ok(());
        }
        let text = (self.load)(&key).map_err(|e| span.error(format!("cannot load {path:?}: {e}")))?;
        let src = Source::new(&key, &text);
        let stmts = parse(&src)?;
        self.loading.push(key.clone());
        let saved = std::mem::replace(&mut self.file, key.clone());
        let global = self.global.clone();
        let r = self.exec(&stmts, &global);
        self.file = saved;
        self.loading.pop();
        r?;
        self.loaded.push(key);
        Ok(())
    }

    // --- calls ---------------------------------------------------------------

    /// Runs body with the depth guard and, if it fails, records the frame.
    fn call<F>(&mut self, c: &Callable, site: &Span, body: F) -> Result<()>
    where
        F: FnOnce(&mut Evaluator) -> Result<()>,
    {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err(site.error(format!("{} {}: call depth exceeds {}", c.kind, c.name, MAX_DEPTH)));
        }
        let r = body(self);
        self.depth -= 1;
        r.map_err(|mut e| {
            let (line, col) = site.src.pos(site.a);
            e.frames.push(format!("{} {} ({}:{}:{})", c.kind, c.name, site.src.name, line, col));
            e
        })
    }

    /// Evaluates a call's arguments in the caller's scope and binds them to
    /// the callable's parameters in a fresh scope under its definition scope.
    fn bind(&mut self, c: &Callable, args: &[Expr], site: &Span, caller: &Env) -> Result<Env> {
        let mut vals = Vec::with_capacity(args.len());
        for a in args {
            vals.push(self.eval(a, caller, false)?);
        }
        let scope = new_env(Some(&c.env));
        for (i, prm) in c.params.iter().enumerate() {
            if prm.rest {
                let rest = if i < vals.len() { vals[i..].to_vec() } else { Vec::new() };
                define(&scope, &prm.name, Value::list(rest, true));
                return Ok(scope);
            }
            let v = if i < vals.len() {
                vals[i].clone()
            } else if let Some(d) = &prm.default {
                self.eval(d, &scope, false)?
            } else {
                return Err(site.error(format!("{} {}: missing argument ${}", c.kind, c.name, prm.name)));
            };
            define(&scope, &prm.name, v);
        }
        if vals.len() > c.params.len() {
            let n = c.params.len();
            let noun = if n == 1 { "1 argument".to_string() } else { format!("{n} arguments") };
            return Err(site.error(format!("{} {} takes {}, got {}", c.kind, c.name, noun, vals.len())));
        }
        Ok(scope)
    }

    fn call_function(&mut self, c: &Rc<Callable>, args: &[Expr], site: &Span, caller: &Env) -> Result<Value> {
        let scope = self.bind(c, args, site, caller)?;
        let mut result = Value::Null;
        self.call(c, site, |ev| {
            if let Some(f) = c.builtin {
                let vals: Vec<Value> = c.params.iter().map(|p| lookup(&scope, &p.name).unwrap_or(Value::Null)).collect();
                result = f(&vals).map_err(|e| site.error(format!("{}(): {e}", c.name)))?;
                return Ok(());
            }
            let out = std::mem::take(&mut ev.out);
            ev.in_func += 1;
            ev.ret = None;
            let returned = ev.exec(&c.body, &scope);
            ev.in_func -= 1;
            ev.out = out;
            if !returned? {
                return Err(site.error(format!("function {} did not return a value", c.name)));
            }
            result = ev.ret.take().unwrap_or(Value::Null);
            Ok(())
        })?;
        Ok(result)
    }

    // --- expressions ---------------------------------------------------------

    /// Raw text with its #{...} holes filled.
    fn interp(&mut self, parts: &Interp, env: &Env) -> Result<String> {
        let mut out = String::new();
        for p in parts {
            match p {
                Part::Text(t) => out.push_str(t),
                Part::Hole(e) => {
                    let v = self.eval(e, env, false)?;
                    let s = str_of(&v).map_err(|m| e.span().error(format!("cannot interpolate: {m}")))?;
                    out.push_str(&s);
                }
            }
        }
        Ok(out)
    }

    fn css_text(&self, at: &Span, v: &Value) -> Result<String> {
        css(v).map_err(|m| at.error(m))
    }

    /// Evaluates an expression. In CSS value context (a declaration value or
    /// the arguments of a CSS function) an operator chain with no computed
    /// operand is not evaluated at all but emitted as written, `/` is never
    /// division, and an operation that has no result is kept symbolically.
    pub fn eval(&mut self, x: &Expr, env: &Env, css_ctx: bool) -> Result<Value> {
        self.step(x.span())?;
        match x {
            Expr::Num { span, val, unit } => Ok(Value::Num { v: *val, unit: unit.as_str().into(), text: Some(span.text().into()) }),
            Expr::Str { parts, quote, .. } => Ok(Value::Str { s: self.interp(parts, env)?.into(), quote: *quote }),
            Expr::Var { span, name } => lookup(env, name).ok_or_else(|| span.error(format!("undefined variable ${name}"))),
            Expr::Lit { val, .. } => Ok(val.clone()),
            Expr::List { items, comma, .. } => {
                let mut vals = Vec::with_capacity(items.len());
                for it in items {
                    vals.push(self.eval(it, env, css_ctx)?);
                }
                Ok(Value::list(vals, *comma))
            }
            Expr::Map { span, pairs } => {
                let mut m: Vec<(Value, Value)> = Vec::new();
                for (k, v) in pairs {
                    let k = self.eval(k, env, false)?;
                    let v = self.eval(v, env, false)?;
                    match map_set(&m, k, v).map_err(|e| span.error(e))? {
                        Value::Map(nm) => m = nm.as_ref().clone(),
                        _ => unreachable!(),
                    }
                }
                Ok(Value::Map(Rc::new(m)))
            }
            Expr::Paren { x, .. } => {
                if !css_ctx {
                    return self.eval(x, env, false);
                }
                // Parentheses in a value are the request to compute: inside
                // them `/` divides. What still cannot be computed keeps its
                // parentheses, so calc((100% - $x) / 2) comes out right.
                let v = self.operate(x, env, true, true)?;
                Ok(match &v {
                    Value::Str { s, quote } => Value::unquoted(format!("({})", css_string(s, *quote))),
                    _ => v,
                })
            }
            Expr::Call { span, name, args, .. } => {
                if let Some(c) = find_function(env, name) {
                    return self.call_function(&c, args, span, env);
                }
                self.css_call(x, env)
            }
            Expr::Binary { .. } | Expr::Unary { .. } => {
                if css_ctx && !self.computed(x, env) {
                    return Ok(Value::unquoted(strip_comments(x.span().text())));
                }
                self.operate(x, env, css_ctx, !css_ctx)
            }
        }
    }

    /// Whether an expression contains something the author asked to
    /// compute: a variable, a lace function call, parentheses, a map, or
    /// interpolation. Plain CSS like `12px/1.5` or `var(--x,)` has none.
    fn computed(&self, x: &Expr, env: &Env) -> bool {
        match x {
            Expr::Var { .. } | Expr::Paren { .. } | Expr::Map { .. } => true,
            Expr::Call { name, args, raw, .. } => {
                find_function(env, name).is_some()
                    || args.iter().any(|a| self.computed(a, env))
                    || raw.as_ref().map_or(false, |r| r.iter().any(|p| matches!(p, Part::Hole(_))))
            }
            Expr::Str { parts, .. } => parts.iter().any(|p| matches!(p, Part::Hole(_))),
            Expr::List { items, .. } => items.iter().any(|it| self.computed(it, env)),
            Expr::Binary { l, r, .. } => self.computed(l, env) || self.computed(r, env),
            Expr::Unary { x, .. } => self.computed(x, env),
            _ => false,
        }
    }

    /// Evaluates an operator chain. With symbolic set (CSS value context) an
    /// operation that is not defined, or that involves text, is kept as
    /// written with its operands filled in instead of failing, because inside
    /// calc() that is exactly right. With divide unset, `/` is always kept: in
    /// a value it is CSS's separator, as in `grid-area: $r / $c`.
    fn operate(&mut self, x: &Expr, env: &Env, symbolic: bool, divide: bool) -> Result<Value> {
        match x {
            Expr::Unary { span, op, x: inner } => {
                let v = self.sub(inner, env, symbolic, divide)?;
                match (op, &v) {
                    (UnOp::Not, _) => return Ok(Value::Bool(!v.truthy())),
                    (UnOp::Plus, Value::Num { .. }) => return Ok(v),
                    (UnOp::Neg, Value::Num { v: n, unit, .. }) => return Ok(Value::num(-n, unit)),
                    (UnOp::Neg, Value::Str { s, quote }) => return Ok(Value::unquoted(format!("-{}", css_string(s, *quote)))),
                    _ => {}
                }
                let sign = if *op == UnOp::Neg { '-' } else { '+' };
                if symbolic {
                    return Ok(Value::unquoted(format!("{sign}{}", self.css_text(span, &v)?)));
                }
                Err(span.error(format!("cannot apply unary {sign} to {}", inspect(&v))))
            }
            Expr::Binary { span, op, l, r } => {
                let lv = self.sub(l, env, symbolic, divide)?;
                match op {
                    Op::And => return if !lv.truthy() { Ok(lv) } else { self.sub(r, env, symbolic, divide) },
                    Op::Or => return if lv.truthy() { Ok(lv) } else { self.sub(r, env, symbolic, divide) },
                    _ => {}
                }
                let rv = self.sub(r, env, symbolic, divide)?;
                // Text next to an operator is CSS's operator, except that
                // inside parentheses (the request to compute) `+` still
                // concatenates.
                let text_involved = op.is_arith() && (lv.is_string() || rv.is_string()) && !(divide && *op == Op::Add);
                if symbolic && ((*op == Op::Div && !divide) || text_involved) {
                    return self.symbolic(span, l, r, &lv, &rv);
                }
                match binary(*op, &lv, &rv) {
                    Ok(v) => Ok(v),
                    Err(_) if symbolic => self.symbolic(span, l, r, &lv, &rv),
                    Err(m) => Err(span.error(m)),
                }
            }
            other => self.eval(other, env, symbolic),
        }
    }

    /// Operands of a chain that is being evaluated.
    fn sub(&mut self, y: &Expr, env: &Env, symbolic: bool, divide: bool) -> Result<Value> {
        match y {
            Expr::Binary { .. } | Expr::Unary { .. } => self.operate(y, env, symbolic, divide),
            _ => self.eval(y, env, symbolic),
        }
    }

    /// An operation as written, with its operands filled in and the author's
    /// spacing around the operator.
    fn symbolic(&self, span: &Span, l: &Expr, r: &Expr, lv: &Value, rv: &Value) -> Result<Value> {
        let between = l.span().between(r.span());
        let mut op = between.trim().to_string();
        if op != between {
            // Keep one space on each side that had any: calc() needs them.
            if between.starts_with(|c: char| c.is_whitespace()) {
                op.insert(0, ' ');
            }
            if between.ends_with(|c: char| c.is_whitespace()) {
                op.push(' ');
            }
        }
        Ok(Value::unquoted(format!("{}{}{}", self.css_text(span, lv)?, op, self.css_text(span, rv)?)))
    }

    /// Passes an unknown function through with its arguments evaluated, or
    /// as written when there is nothing to evaluate in them.
    fn css_call(&mut self, x: &Expr, env: &Env) -> Result<Value> {
        let Expr::Call { span, name, args, raw } = x else { unreachable!() };
        if !self.computed(x, env) {
            return Ok(Value::unquoted(strip_comments(span.text())));
        }
        if let Some(raw) = raw {
            let inner = self.interp(raw, env)?;
            return Ok(Value::unquoted(format!("{name}({inner})")));
        }
        let mut parts = Vec::new();
        for a in args {
            // A null argument is left out, like a null item in a list.
            let v = self.eval(a, env, true)?;
            if !v.is_null() {
                parts.push(self.css_text(a.span(), &v)?);
            }
        }
        Ok(Value::unquoted(format!("{name}({})", parts.join(", "))))
    }
}

fn clone_callable(c: &Callable) -> Callable {
    Callable {
        kind: c.kind,
        name: c.name.clone(),
        params: c.params.clone(),
        body: c.body.clone(),
        env: c.env.clone(),
        builtin: c.builtin,
        locked: c.locked,
        content: c.content,
    }
}

/// Applies an operator to two values, or explains why it cannot.
fn binary(op: Op, l: &Value, r: &Value) -> std::result::Result<Value, String> {
    match op {
        Op::Eq => return Ok(Value::Bool(equal(l, r))),
        Op::Ne => return Ok(Value::Bool(!equal(l, r))),
        _ => {}
    }
    if let (Value::Num { .. }, Value::Num { .. }) = (l, r) {
        return num_op(op.text(), l, r);
    }
    let l_str = matches!(l, Value::Str { .. });
    let r_str = matches!(r, Value::Str { .. });
    let l_ok = l_str || matches!(l, Value::Num { .. });
    let r_ok = r_str || matches!(r, Value::Num { .. });
    if op == Op::Add && (l_str || r_str) && l_ok && r_ok {
        let (lt, rt) = (str_of(l)?, str_of(r)?);
        if lt.len() + rt.len() > MAX_STRING_LEN {
            return Err("string too large".into());
        }
        let quote = match l {
            Value::Str { quote, .. } => *quote,
            _ => None,
        };
        return Ok(Value::Str { s: format!("{lt}{rt}").into(), quote });
    }
    Err(format!("{} {} {}: undefined operation", inspect(l), op.text(), inspect(r)))
}

/// Whether a mixin body reaches an @content statement.
fn uses_content(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| match s {
        Stmt::Content { .. } => true,
        Stmt::Rule { body, .. } | Stmt::Each { body, .. } | Stmt::For { body, .. } | Stmt::While { body, .. } => uses_content(body),
        Stmt::AtRule { body: Some(body), .. } => uses_content(body),
        Stmt::Include { body: Some(body), .. } => uses_content(body),
        Stmt::If { then, otherwise, .. } => uses_content(then) || uses_content(otherwise),
        _ => false,
    })
}

/// Resolves a @use path against the directory of the file that uses it,
/// with forward slashes and `.` / `..` segments folded.
fn resolve(from: &str, path: &str) -> String {
    let from = from.replace('\\', "/");
    let dir = match from.rfind('/') {
        Some(i) => &from[..i],
        None => "",
    };
    let joined = if dir.is_empty() { path.replace('\\', "/") } else { format!("{dir}/{}", path.replace('\\', "/")) };
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if matches!(parts.last(), Some(&p) if p != "..") {
                    parts.pop();
                } else {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let mut out = parts.join("/");
    if joined.starts_with('/') {
        out.insert(0, '/');
    }
    out
}

/// Removes comments from a piece of source text that is being emitted as
/// written, and folds whitespace runs to one space.
pub fn strip_comments(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut space = false;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'\\' && i + 1 < b.len() {
            if space && !out.is_empty() {
                out.push(b' ');
            }
            space = false;
            out.extend_from_slice(&b[i..i + 2]);
            i += 2;
            continue;
        }
        if c == b'"' || c == b'\'' {
            let mut j = i + 1;
            while j < b.len() && b[j] != c {
                if b[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            let j = j.min(b.len() - 1);
            if space && !out.is_empty() {
                out.push(b' ');
            }
            space = false;
            out.extend_from_slice(&b[i..=j]);
            i = j + 1;
            continue;
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            match s[i + 2..].find("*/") {
                Some(end) => i += end + 4,
                None => break,
            }
            space = true;
            continue;
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            space = true;
            continue;
        }
        if is_space(c) {
            space = true;
            i += 1;
            continue;
        }
        if space && !out.is_empty() {
            out.push(b' ');
        }
        space = false;
        out.push(c);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Error {
    pub fn is_budget(&self) -> bool {
        self.msg.starts_with("evaluation budget")
    }
}
