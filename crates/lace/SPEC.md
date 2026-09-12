# lace - CSS plus computation

lace is a small language for generating CSS. It is CSS with variables,
functions, mixins, conditionals and loops added, and nothing else. Anything
CSS can already do (nesting, `&`, custom properties, `calc()`, `min()`,
nested `@media`) is left to CSS and passes through untouched.

The name is a placeholder. Files use the `.scss` extension so editors
highlight them; the syntax is a subset of SCSS with a few deliberate
differences, listed in section 12.

Two invariants:

1. **CSS in, CSS out.** A plain CSS file compiles to itself, modulo
   whitespace and comments. Computation happens only where the source asks
   for it: `$variables`, `#{interpolation}`, calls of lace functions,
   parentheses, and the language's `@`-rules.
2. **Turing complete.** `@while` and lists of unbounded length give a
   counter machine; `@function` gives recursion. Numbers are float64, so
   unbounded storage comes from lists, not from number magnitude. A step
   budget turns an infinite loop into an error instead of a hang.

## 1. Lexical structure

- Source is UTF-8. A leading byte order mark is skipped; CR, CRLF and FF
  are read as LF. Whitespace is space, tab and LF. Bytes outside ASCII are
  identifier characters.
- `// comment` runs to end of line and is removed before anything else
  looks at the text. It is not a comment inside a quoted string or inside
  an unquoted `url(...)`. Note that this applies to custom property values
  too: `--home: https://x.y;` loses its tail; write `url(https://x.y)` or
  quote it.
- `/* comment */` is kept in the output when it stands where a statement
  could; anywhere else it is removed.
- Identifiers start with `--`, with `#` (a hash colour: `#fff`, `#1e3`),
  with `#{...}`, or with an optional `-` followed by a letter, `_`, `\`,
  a non-ASCII byte or `#{...}`; they continue with `[A-Za-z0-9_-]`, `\`
  escapes (a backslash and the next character, kept verbatim), non-ASCII
  bytes and `#{...}` pieces. `col-#{$n}`, `#{$x}s` and `#{$x}` are each one
  identifier.
- Variables: `$` followed by identifier characters, greedily: `$a-$b` is
  the variable `$a-` followed by `$b`; write `$a - $b` to subtract, and
  `$a-1` names the variable `a-1`.
- Numbers: `(digits [. digits]? | . digits) ([eE] [+-]? digits)?` followed
  by an optional unit, `%` or letters. Numbers carry no sign; `-` and `+`
  are operators (section 4). `e` begins an exponent only when followed by a
  digit or a sign and a digit, so `1em` is `1` with unit `em`.
- Strings: `"..."` or `'...'`, on one line. Backslash escapes are kept
  verbatim (they are CSS escapes, not lace escapes). `#{...}` inside a
  string is interpolated. A string keeps the quote character it was written
  with.
- `url(` followed by anything other than a quote consumes raw text up to
  the first `)`, `#{...}` inside it interpolated.
- `[` ... `]` in a value is raw text up to the matching `]`, `#{...}`
  interpolated, and is an unquoted string: grid line names pass through as
  written.
- `u+` or `U+` followed by up to six hex digits or `?`, optionally `-` and
  up to six more, is a unicode-range token: an unquoted string as written.

## 2. Statements

A file is a sequence of statements. A block is `{` statements `}`. The
`;` after the last statement in a block is optional; empty statements are
ignored. Statements are emitted in source order: lace never reorders
declarations and nested rules.

A statement is classified in this order: `/*` starts a comment; `@` a
lace rule or a CSS at-rule; `$` an assignment; `--` a custom property;
otherwise the text is scanned to the first `;`, `{` or `}` at depth zero,
skipping strings, `\` escapes, comments, parentheses, brackets and
`#{...}` as units, and a `{` first means a rule.

### CSS statements

- **Declaration**: `name: value;`, optionally ending in `!important` in
  any spelling (`! important`, `!IMPORTANT`), emitted as written. `name` is
  an identifier (interpolation allowed). `value` is parsed in *CSS value
  context* (section 5). If the value evaluates to `null`, to `()`, or to
  a list whose items are all `null`, the declaration is omitted. A
  declaration outside every block is an error, and so is a declaration
  whose name does not start like an identifier (`*zoom: 1`).
- **Custom property**: a declaration whose name starts with `--`. The value
  is raw text up to the terminating `;` (or the `}` that closes the block)
  with balanced braces allowed inside; only `#{...}` is evaluated in it.
  Whitespace at either end is trimmed, interior whitespace is collapsed to
  single spaces; `--x: ;` is emitted as written.
- **Rule**: `selector { block }`. The selector is raw text with
  interpolation, whitespace collapsed, taken up to the `{`. lace does not
  parse selectors; `&` and nesting are handed to the browser as written
  (CSS Nesting, in every browser since 2023). Two things that are not
  selectors are rejected: a `:` followed by a space or ending the selector
  at depth zero (a declaration that lost its `;`, or Sass's nested
  properties `font: { ... }`), and `&` directly followed by an
  identifier character (`&__title` is Sass, not CSS; write `.card__title`
  flat or from a variable: `#{$b}__title`).
- **At-rule**: `@name prelude;` or `@name prelude { block }` for any
  `@name` that is not a lace rule below. The prelude is raw text with
  interpolation. The block is parsed as statements, so `@media`,
  `@supports`, `@container`, `@layer`, `@property`, `@font-face`,
  `@keyframes` (`from`, `to` and `50%` parse as rules) and future at-rules
  all work without lace knowing them. `@import` and `@charset` pass
  through. `@function --name` and `@mixin --name`, CSS's own, pass through
  too.

A rule whose block produces no output is dropped. An at-rule block the
author wrote empty (`@layer base {}`, which fixes the layer's place in the
cascade) is emitted as `@layer base {}`; an at-rule whose non-empty block
produced nothing is dropped. Bodiless at-rules are always emitted.

A `$` followed by an identifier character in any raw text (selector,
prelude, custom property value, `[...]`) outside a quoted string is an
error: `use #{$x} to interpolate a variable here`. Plain CSS never
contains one, and passing it through would make the browser drop the rule
silently.

### Language statements

- `@use "path";` - compile another file here. The path is relative to the
  file containing the `@use`, written with its extension. Its output is
  emitted at this point; its top-level variables, mixins and functions
  become global. Each file is loaded once per compilation, keyed by its
  cleaned path; a cycle is an error. `@use` is allowed only at the top
  level of a file.
- `$name: expression;` - assignment (section 6). `$name: expression
  !default;` assigns only if `$name` is undefined or `null`; a skipped
  `!default` does not evaluate its expression. This is how a stylesheet
  configures a file it is about to `@use`: assign first, `@use` second.
- `@mixin name { block }` / `@mixin name(params) { block }` and
  `@include name;` / `@include name(args);` / `@include name(args) { block }`.
  Inside a mixin, `@content;` runs the block passed to the `@include`, in
  the scope of the `@include`; when the mixin was included without a
  block, `@content;` does nothing. `@content` may appear at any depth
  inside the mixin body and refers to the innermost enclosing include.
  Passing a block to a mixin whose body contains no `@content` is an error.
  `@content` outside a mixin is an error.
- `@function name(params) { block }` with `@return expression;`, allowed
  at any depth inside the body; it ends the call. A function body may
  contain only assignments, control flow, `@return`, `@error` and `@debug`;
  producing CSS inside a function is an error, and comments there are
  dropped. Falling off the end is an error ("did not return a value").
- `@if expression { block } @else if expression { block } @else { block }`
- `@each $a in expression { block }` and `@each $a, $b, ... in expression
  { block }`. The value is viewed as a list (section 3). With one variable
  each item is bound to it; with several, each item is itself viewed as a
  list and destructured positionally, missing positions bound to `null`.
  `@each $a in ()` runs zero times; `@each $a in null` runs once.
- `@for $i from a through b { block }` (inclusive) and `@for $i from a to
  b { block }` (exclusive). `a` and `b` are single expressions (no space
  lists), integer-valued, with the same unit or one of them unitless;
  `$i` carries the unit. The loop counts up only: on the k-th iteration
  `$i` is `a + k`, and when `b` is below `a` the body does not run, so
  `from 1 through length($empty)` runs zero times.
- `@while expression { block }`
- `@error expression;` aborts compilation; the message is the string
  without its quotes, or the inspect form of any other value. `@debug
  expression;` prints `file:line:col: debug: text` the same way to the log
  and continues.

Mixins and functions are defined when their definition executes; a call
before the definition sees an unknown name (and an unknown function in a
value is CSS, passed through). Defining a mixin or function whose name is
already defined in the same scope is an error; an inner scope may shadow.
A function may not take the name of a builtin or prelude function.

### Parameters and arguments

`params` is a comma-separated list of `$name` or `$name: default`, with an
optional final `$rest...` collecting the remaining arguments into a comma
list. Defaults are evaluated at call time in the callee's scope and may
refer to earlier parameters.

`args` is a comma-separated list of expressions, each parsed at the
space-list level; wrap a comma list in parentheses to pass it as one
argument: `f((1, 2))`. A trailing comma is allowed in argument and
parameter lists. Too many or too few arguments is an error naming the
callable and the parameter. There are no keyword arguments and no spread
(section 12).

## 3. Values

| type   | literal                                  | notes |
|--------|------------------------------------------|-------|
| number | `12`, `1.5em`, `50%`                     | float64 and a unit string; `""` for unitless |
| string | `"a"`, `'a'`, `a`, `bold-#{$w}`, `#fff` | quoted or unquoted; both are strings |
| bool   | `true`, `false`                          | |
| null   | `null`                                   | |
| list   | `a b c`, `a, b, c`, `(a, b)`, `(a,)`, `()` | items and a separator, space or comma |
| map    | `(key: value, key2: value2,)`            | ordered; keys are any value compared with `==` |

There is no colour type. `#fff`, `red` and `rgb(1 2 3)` are unquoted
strings and functions passed through; colour math is what `calc()`,
`color-mix()` and relative colour syntax are for, and they follow a theme
toggle at runtime, which compile-time maths cannot.

`()` is both the empty list and the empty map: every `map-*` function
accepts it, `type-of(())` is `list`, and `map-set((), $k, $v)` is how a
map is started. A trailing comma is allowed in every parenthesised list
and map; `(a,)` is a one-item comma list.

Every value can be viewed as a list: a list is itself, a map is a comma
list of two-item space lists `key value`, and anything else is a one-item
space list. `length`, `nth`, `append`, `index`, `join` and `@each` use
this view.

Truthiness: `false` and `null` are false; everything else is true,
including `0`, `""` and `()`.

Equality (`==`): values of different types are never equal, except that a
quoted and an unquoted string with the same text are equal. Numbers are
equal when their units are the same and their values differ by less than
1e-10, which is the output precision; the ordering comparisons treat such
numbers as equal too. Lists are equal item-wise; the separator matters
only between lists of two or more items, and a one-item list equals its
item. Maps are equal when they hold the same pairs in any order.

## 4. Expressions

Precedence, lowest to highest:

1. comma list `a, b`
2. space list `a b`
3. `or`
4. `and`
5. `not` (prefix; note it binds looser than the comparisons below)
6. `==` `!=`
7. `<` `>` `<=` `>=`
8. `+` `-`
9. `*` `/` `%`
10. unary `-` `+`
11. primary: literal, `$var`, `(expression)`, `()`, `(k: v, ...)`,
    `[raw]`, `name(args)`, identifier with `#{...}`

`and` and `or` short-circuit and return the deciding operand (`a or b` is
`a` if `a` is truthy, else `b`). In expression context the words `and`,
`or`, `not`, `true`, `false` and `null` are reserved; `in`, `from`, `to`
and `through` are keywords only in `@each` and `@for` headers.

### Operators on numbers

- `+ - * / %` between two numbers. Units: if both have the same unit the
  result keeps it; if exactly one is unitless the result takes the other's
  unit; `*` of two unitful numbers and `+ - % < > <= >=` of two different
  units are errors; `/` of two numbers with the same unit is unitless.
  There is no unit conversion (`1in + 1cm` is an error; write `calc()`).
- `%` is the remainder with the sign of the dividend, as `%` on floats.
- Division by zero, and any result that is infinite or NaN, is an error.
- A number that results from arithmetic is formatted by section 8; a
  literal that was never operated on keeps its spelling.

### Operators on strings

- `+` concatenates when at least one operand is a string and the other is
  a string or a number; the result is quoted iff the left operand is a
  quoted string: `"a" + 1` is `"a1"`, `1 + a` is `1a`, `sans- + serif` is
  `sans-serif`. Quoted operands contribute their text without quotes.
- Unary `-` on a string is the unquoted string `-` followed by its CSS
  text: `-$dir` with `$dir: left` is `-left`.
- `- * / %` and the ordering comparisons between strings, or a string and
  a number, are errors in expression context. In CSS value context they
  are kept as written with the operands filled in (section 5).

Apart from `==`, `!=`, `and`, `or`, `not`, and `+` with a string, any
operator on `null`, a bool, a list or a map is an error; use `inspect()` or
`#{}` to turn one into text.

### `-` and `+`

`-` and `+` are unary when no operand precedes them: at the start of an
expression or argument, after `(`, `,`, `:` or another operator; and also
when whitespace precedes them and none follows. In every other position
they are binary: `1-2`, `1- 2` and `1 - 2` all subtract, `1 -2` is a space
list of `1` and `-2`, `$a -$b` is a space list of `$a` and `-$b`. A sign
directly followed by a digit or `.` is part of the number literal; inside
an identifier `-` is just a character (`margin-top`, `-webkit-x`), and an
identifier swallows its own dashes (section 1).

## 5. CSS value context

Declaration values and the arguments of CSS functions are parsed with the
grammar above and evaluated under these rules, whose purpose is that
plain CSS reaches the output as written:

- An expression is *computed* if it contains a `$variable`, a call of a
  lace function, a parenthesised expression, a map, `#{...}`
  interpolation, or a CSS function call with a computed argument.
- An operator chain (a maximal tree of binary and unary operators) that is
  not computed is emitted exactly as written, comments removed and
  whitespace collapsed: `font: 12px/1.5 serif`, `grid-area: 1 / 3`,
  `margin: 0 -1px`, `content: "a" + b`, `x: not y` and `unicode-range:
  U+0000-00FF` never change. A computed chain is evaluated with two
  differences from expression context:
  - `/` is never division; it is kept as written with its operands filled
    in: `font: $fs/$lh serif` is `16px/1.5 serif`, `rgb(0 0 0 / $a)` is
    `rgb(0 0 0 / 0.5)`. Division in a declaration is written in
    parentheses: `width: ($w / 3)`.
  - An operation that has no result (different units, an operand that is
    text, `+` next to text) is kept as written with its operands filled
    in and the author's spacing around the operator: `calc(100% - $x)`
    is `calc(100% - 2px)`, `calc(var(--w) + $gap)` is
    `calc(var(--w) + 8px)`, `#{$a}/2` is `1fr/2`.
- A parenthesised expression is the request to compute: inside it `/`
  divides and the operations above are attempted. What still cannot be
  computed keeps its parentheses, so `calc((100% - $x) / 2)` is
  `calc((100% - 2px) / 2)`; a parenthesised expression whose value is a
  string keeps its parentheses, one whose value is a number does not.
- A number literal keeps its source spelling until arithmetic produces a
  new number, through variables too: `$d: .55s; animation: rise $d;`
  emits `.55s`, while `$d * 2` emits `1.1s`. `true` and `false` print as
  themselves (`inherits: false` in `@property`); `null` omits the
  declaration.
- A call of a name that is not a lace function is a CSS function. It is
  passed through with its arguments evaluated in CSS value context and
  joined with `, `; an argument list with nothing computed in it is
  emitted as written, so `var(--x,)` keeps its empty fallback. An
  argument list containing `:` or `;` outside nested parentheses and
  strings is raw text in which only `#{...}` is evaluated, which is how
  `if(style(--wide: true): 100%; else: 50%)` and whatever CSS invents next
  pass through. This holds in expression context too: `$shadow: 0 0 4px
  rgba(0, 0, 0, .2)` and `$w: calc(100% - $gutter)` store the CSS text.
- Neither builtins nor prelude ever take the name of a CSS function.
  `min()`, `max()`, `clamp()`, `round()`, `abs()` and the rest of CSS math
  are CSS's job: `width: min($a, 3px)` passes through as `min(2px, 3px)`
  and the browser computes it. A stylesheet that needs one of those at
  compile time defines its own `@function` with that name; user
  definitions win.

The consequence to remember: `width: 100% / 3` passes through unchanged,
because nothing in it asked to be computed. Write `(100% / 3)`.

Everything else in a stylesheet (selectors, at-rule preludes, custom
property values, `[...]`) is raw text where only `#{...}` is evaluated.

## 6. Scope

- Every block introduces a scope. Top level is the global scope; files
  loaded with `@use` share it. The prelude is loaded into it first.
- `$x: v` assigns to the nearest enclosing scope that already defines
  `$x`, otherwise defines `$x` in the current scope. There is no
  `!global`. Because `@if` and loop bodies are scopes, a variable that
  must outlive them is declared before them: `$bg: null; @if $dark { $bg:
  black; } @else { $bg: white; }`.
- Reading a variable that no enclosing scope defines is an error.
- Variables, mixins and functions live in three separate namespaces.
- Mixins and functions are closures over the scope they were defined in.
  A call runs in a fresh scope whose parent is the definition scope, with
  the parameters bound. So an assignment inside a mixin or function to a
  name defined only at top level updates the global (this is how a mixin
  counts), while parameters and names first assigned inside the body are
  local. A `@content` block runs in a fresh scope whose parent is the
  scope of the `@include`.
- Each iteration of `@each`, `@for` and `@while` runs the body in a fresh
  scope under the loop's enclosing scope, with the loop variables defined
  in it; a variable first assigned inside the body does not survive the
  iteration.

## 7. Interpolation

`#{expression}` may appear in identifiers (and so in declaration values),
quoted strings, selectors, at-rule preludes, custom property values,
`url(...)` and `[...]`. The expression is evaluated in expression context
and its CSS text is spliced in, never re-lexed: `#{$n}px` is the string
`12px`, not a number, and `#{$latin}` is one string although it contains
commas. Only a value that is itself a quoted string loses its quotes;
strings inside lists keep them, so `--stack: #{$stack}` with `$stack:
"Public Sans", sans-serif` is `--stack: "Public Sans", sans-serif`. A
bool splices as `true`/`false`; `null` and `()` splice nothing; a map is
an error. Spliced text is not escaped, even inside a quoted string.

## 8. Serialisation

- Output is expanded CSS: blocks are `prelude {`, contents indented by two
  spaces per level, `}` on its own line, an empty kept block as `prelude
  {}`; declarations are `name: value;`, one per line. Top-level statements
  are separated by one blank line. Comments are emitted verbatim on their
  own line.
- Computed numbers print with ten decimals,
  with trailing zeros and then a trailing `.` removed, `-0` as `0`, then
  the unit: `2/3` is `0.6666666667`, `1e21` is `1000000000000000000000`,
  `1e-11` is `0`. Literal numbers print as written.
- Strings: quoted strings keep their quotes; unquoted strings are emitted
  raw. Bools print as `true`/`false`.
- Lists: items separated by a single space or by `, `; `null` items are
  left out. An empty list, a map, or `null` outside a declaration value is
  an error when it reaches CSS output; all are fine as data.
- `@debug` and `@error` use the *inspect* form for non-strings: lists in
  parentheses when they have fewer than two items (`(a)`, `(a,)`, `()`),
  maps as `(k: v, ...)`, `null` as `null`.

## 9. Builtins and prelude

Builtins are the operations the language cannot express about its own
values, plus three it could express but not in constant time (`length`,
`nth`, `map-get`). Everything else is a prelude written in lace itself,
embedded in the compiler and loaded before the entry file. Both are always
available and neither can be redefined.

Builtins:

| function | |
|---|---|
| `length($list)` | number of items (of pairs, for a map) |
| `nth($list, $n)` | 1-based; negative counts from the end; 0 or out of range is an error |
| `append($list, $value, $separator: auto)` | new list; `auto` keeps the list's separator (a one-item list keeps the one it was built with; `()` and non-lists count as space), `comma`/`space` force one |
| `map-get($map, $key)` | value or `null` |
| `map-set($map, $key, $value)` | new map with the pair added or replaced, order preserved |
| `type-of($value)` | `number`, `string`, `bool`, `null`, `list`, `map` |
| `unit($number)` | the unit as a quoted string, `""` for unitless |
| `inspect($value)` | the inspect form as an unquoted string |

Prelude (lace): `floor`, `ceil`, `percentage`, `unitless`, `quote`,
`unquote`, `join($a, $b, $separator: auto)`, `index($list, $value)`,
`map-has-key`, `map-keys`, `map-values`, `map-remove`, `map-merge($a,
$b)`. Its source is printed by `lace -prelude` and is part of the test
suite. Round-half-up is `floor($n + 0.5)`; compile-time min and max are
`@if $a < $b`.

## 10. Errors

Every error is `file:line:column: message` (column in bytes) on stderr
and exit status 1. Runtime errors carry the location of the expression or
statement being evaluated and, when inside includes or calls, one line per
frame naming the call site: `  in mixin card (theme.scss:40:3)`. Messages
name the thing at fault (`unknown mixin "crad"`, `mixin card: missing
argument $size`, `1px + 2em: incompatible units`, `undefined variable
$bg`).

Limits, each an error rather than a crash or a hang: the evaluation
budget, one step per statement executed and per expression evaluated,
10,000,000 by default (`-budget N`; `Compiler.Budget`; negative for
unlimited); include and call depth 1,000; syntactic nesting of blocks,
parentheses, brackets, arguments and interpolation 500 levels; strings
above 1 MiB and lists or maps above 1,000,000 items. An unterminated
string, comment or block is an error naming where it started.

lace does not implement CSS error recovery: `*zoom: 1`, `filter:
progid:...`, `<!-- -->` and other pre-2010 hacks are parse errors.

## 11. Command line and API

    lace input.scss              # CSS on stdout
    lace input.scss -o out.css   # flags may appear anywhere
    lace -prelude                # print the prelude
    lace < input.scss            # stdin, @use resolves against the cwd
    lace -budget -1 input.scss   # no step budget

Rust API in the `lace` crate, no dependencies:

    lace::Compiler::new().load(loader).log(writer).budget(n).compile(src, name)
    lace::compile(src, name)          // the defaults: files from disk, @debug to stderr
    lace::compile_file(path)
    lace::prelude()

`compile` returns `Result<String, lace::Error>`. `name` labels the source
in errors and anchors relative `@use` paths; the cleaned joined path is
passed to the loader and is the load-once key. `Error` has `file`, `line`,
`col`, `msg` and `frames`.

## 12. Not in the language, on purpose

`@extend` and placeholders; a colour type and colour functions; unit
conversion and compound units; nested properties; `!global`; `@use ... as`,
`@forward`, module namespaces; `@import` as inclusion (it is plain CSS);
`@warn` (use `@debug`); keyword arguments and argument spreads (positional
arguments with defaults cover a site's mixins; `$rest...` stays); string
functions (`str-length`, `str-slice`, `str-index`: interpolation builds
every name a stylesheet needs; added back the day a real stylesheet needs
one); bracketed lists as a value type; the indented syntax; source maps;
compressed output; selector functions and `@at-root`; `@media` query
merging and `@supports` manipulation; special-casing of `calc()`,
`min()`, `max()`, `clamp()`, `env()`, `var()`; division-slash legacy
rules; `&-suffix` selector concatenation.

Differences from SCSS a reader of `.scss` files should know: `/` in a
declaration divides only inside parentheses; an operator chain with no
computed operand is never evaluated (`width: 100% / 3` passes through);
literal numbers keep their spelling; `not` binds looser than comparisons;
`$a -$b` is a space list; `$a-$b` is two tokens; a one-item list equals its
item; `!default` also replaces `null`; assignment in a mixin can update a
global; `@for` counts up only; `$x` in selectors, preludes and custom
properties is an error rather than text; a block passed to a mixin without
`@content` is an error; `min`/`max`/`round`/`abs` are CSS functions.
