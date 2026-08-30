//! The syntax tree. Statements produce CSS or bind names; expressions
//! produce values. Raw text regions (selectors, at-rule preludes, custom
//! property values, unquoted url()) are text with #{...} holes.

use std::rc::Rc;

use crate::source::Span;
use crate::value::Value;

/// Raw text with interpolation: each part is text or a hole.
pub type Interp = Vec<Part>;

pub enum Part {
    Text(String),
    Hole(Expr),
}

pub struct Param {
    pub name: String,
    pub default: Option<Expr>,
    pub rest: bool,
}

/// Mixin, function and content bodies are shared with the closures that
/// capture them.
pub type Body = Rc<Vec<Stmt>>;

pub enum Stmt {
    /// `name: value;`. `raw` is set instead of `value` for custom
    /// properties, whose value is text that only interpolation touches.
    Decl { span: Span, name: Interp, value: Option<Expr>, raw: Option<Interp>, important: String },
    /// `selector { body }`; lace never parses the selector.
    Rule { span: Span, selector: Interp, body: Vec<Stmt> },
    /// Any @-rule the language does not own.
    AtRule { span: Span, name: String, prelude: Interp, body: Option<Vec<Stmt>> },
    Comment { span: Span, text: String },
    Use { span: Span, path: String },
    Assign { span: Span, name: String, value: Expr, default: bool },
    Mixin { span: Span, name: String, params: Rc<Vec<Param>>, body: Body },
    Include { span: Span, name: String, args: Vec<Expr>, body: Option<Body> },
    Content { span: Span },
    Function { span: Span, name: String, params: Rc<Vec<Param>>, body: Body },
    Return { span: Span, value: Expr },
    /// One branch; an `@else if` is an If nested in `otherwise`.
    If { span: Span, cond: Expr, then: Vec<Stmt>, otherwise: Vec<Stmt> },
    Each { span: Span, vars: Vec<String>, list: Expr, body: Vec<Stmt> },
    For { span: Span, var: String, from: Expr, to: Expr, inclusive: bool, body: Vec<Stmt> },
    While { span: Span, cond: Expr, body: Vec<Stmt> },
    /// @error or @debug.
    Log { span: Span, kind: LogKind, value: Expr },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Error,
    Debug,
}

impl Stmt {
    pub fn span(&self) -> &Span {
        match self {
            Stmt::Decl { span, .. }
            | Stmt::Rule { span, .. }
            | Stmt::AtRule { span, .. }
            | Stmt::Comment { span, .. }
            | Stmt::Use { span, .. }
            | Stmt::Assign { span, .. }
            | Stmt::Mixin { span, .. }
            | Stmt::Include { span, .. }
            | Stmt::Content { span }
            | Stmt::Function { span, .. }
            | Stmt::Return { span, .. }
            | Stmt::If { span, .. }
            | Stmt::Each { span, .. }
            | Stmt::For { span, .. }
            | Stmt::While { span, .. }
            | Stmt::Log { span, .. } => span,
        }
    }
}

pub enum Expr {
    /// The span is also the spelling: a literal the author wrote is emitted
    /// as written until arithmetic produces a new number.
    Num { span: Span, val: f64, unit: String },
    /// A quoted string (`quote` is the quote character) or an unquoted
    /// identifier-like word, either possibly holding #{...}.
    Str { span: Span, parts: Interp, quote: Option<char> },
    Var { span: Span, name: String },
    Lit { span: Span, val: Value },
    List { span: Span, items: Vec<Expr>, comma: bool },
    Map { span: Span, pairs: Vec<(Expr, Expr)> },
    Binary { span: Span, op: Op, l: Box<Expr>, r: Box<Expr> },
    Unary { span: Span, op: UnOp, x: Box<Expr> },
    /// `raw` is set when the argument text is CSS lace cannot parse.
    Call { span: Span, name: String, args: Vec<Expr>, raw: Option<Interp> },
    Paren { span: Span, x: Box<Expr> },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

impl Op {
    pub fn text(self) -> &'static str {
        match self {
            Op::Add => "+",
            Op::Sub => "-",
            Op::Mul => "*",
            Op::Div => "/",
            Op::Mod => "%",
            Op::Eq => "==",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Gt => ">",
            Op::Le => "<=",
            Op::Ge => ">=",
            Op::And => "and",
            Op::Or => "or",
        }
    }

    pub fn is_arith(self) -> bool {
        matches!(self, Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    Neg,
    Plus,
    Not,
}

impl Expr {
    pub fn span(&self) -> &Span {
        match self {
            Expr::Num { span, .. }
            | Expr::Str { span, .. }
            | Expr::Var { span, .. }
            | Expr::Lit { span, .. }
            | Expr::List { span, .. }
            | Expr::Map { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Call { span, .. }
            | Expr::Paren { span, .. } => span,
        }
    }
}
