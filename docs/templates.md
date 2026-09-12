# press templates - HTML plus computation

The theme of a press site is a directory of HTML files with four things HTML
cannot do: insert values, repeat, choose, and compose. Everything else is the
host language's job: press computes dates, counts, URLs, groupings and links
between pages in Rust and hands them to the template as values. There are no
filters, no macros, no inheritance chains, no arithmetic beyond `+`.

Files are `theme/*.html`. Files starting with `_` are partials; `base.html`
is the shell the page templates extend; every other file is a page
template. This document is the whole language.

## 1. Tags

| tag | meaning |
|---|---|
| `{{ expr }}` | insert the value's text, HTML-escaped (html values as they are) |
| `{% ... %}` | a statement, section 2 |
| `{# ... #}` | a comment, removed |

Outside a tag only the three openers are special, wherever they appear -
inside `<script>`, `<style>`, HTML comments and attribute values included;
the lexer does not read HTML. Inside a tag the text is read as tokens and
the closer is recognised only between tokens, so `{{ "}}" }}` inserts `}}`.
Whitespace inside a tag includes newlines. A comment runs to the first
`#}`; comments do not nest and tags inside them are not recognised. A tag
with no closer is an error at its opener. To emit a literal `{{`, `{%` or
`{#`, insert it as a string: `{{ "{{" }}`. Inserted values are never
scanned for tags.

Source is read as UTF-8; a leading BOM is skipped and CR and CRLF are read
as LF, so output uses LF.

**Standalone lines.** A line whose text, apart from one or more `{% %}` and
`{# #}` tags, is only spaces and tabs is removed whole - leading
whitespace, tags, the whitespace between and after them, and its line
ending - so statements do not leave blank lines behind. A tag that spans
lines counts as standalone when the line it opens on and the line it
closes on are both otherwise blank; they are removed as one. A line that
contains any other text or a `{{ }}` tag is copied as written.

## 2. Statements

Every statement with a body ends with `{% end %}`; `end` takes no keyword.

- `{% if expr %} ... {% else if expr %} ... {% else %} ... {% end %}` -
  one `else` per `if`; `else` belongs to `if` only.
- `{% for x in expr %} ... {% end %}` and `{% for i, x in expr %}`, `i`
  counting from 0. `expr` must be a list; `null` iterates zero times;
  anything else is an error. Each iteration is a scope.
- `{% let x = expr %}` - names a value for the rest of the enclosing body.
  A name that is already visible (a global, an argument, a loop variable,
  another `let`) cannot be declared again; that is an error. Grouping or
  accumulating across a loop is the host's job.
- `{% include "_post_row.html" post = p, index = i + 1 %}` - renders a
  partial with the named arguments as its only variables, plus the
  globals. The file name is a string literal naming a partial (`_*.html`).
  Arguments are evaluated left to right in the including scope; the same
  name twice, an argument the partial never reads, an argument the partial
  reads but was not given, a cycle of includes, and an include of anything
  but a partial are errors. A partial's text is copied verbatim, final
  newline included.
- `{% extend "base.html" title = expr, description = expr %}` - the first
  statement of a page template (only whitespace and comments may precede
  it). The rest of the file is the body. The base renders with the globals
  and the arguments as its variables; its single `{% yield %}` inserts the
  body, rendered with the page template's own variables. `extend` in a
  partial or in the base, a base that extends, `yield` anywhere but in a
  file some template extends, and a second `yield` are errors. The
  arguments are evaluated after the body has rendered, in the page
  template's scope, so they may use names the body `let`s. A page template
  without `extend` renders on its own.

## 3. Expressions

Precedence, lowest to highest: `or`, `and`, `not`, `==` `!=`, `+`, then
postfix `.name` and `(args)`. Binary operators associate to the left;
parentheses group: `(a or b) and c`.

- Literals: `"string"` (one line; `\"` and `\\` escapes), integers, `true`,
  `false`. There is no `null` literal: `null` is only ever produced by
  missing data, and `or` and `if` handle it.
- Names are `[A-Za-z_][A-Za-z0-9_]*`. A front matter key with a dash is not
  reachable; name it with an underscore.
- `x.name` reads a field of an object or a key of a map (section 4).
- `f(args)` calls a function of section 5; only a bare function name can
  be called.
- `+` adds two numbers or joins two strings; anything else, a string and a
  number included, is an error - insert them separately: `rise-{{ i }}`.
- `==` and `!=` compare two values of the same type: numbers, strings by
  code point, dates by day, bools. Different types are an error, and so
  are lists and objects: compare a field (`p.url == page.url`).
- `and` and `or` short-circuit and return the deciding operand: `a or b` is
  `a` when `a` is true, else `b`; `a and b` is `a` when `a` is false, else
  `b`. `not` returns a bool. This is how a fallback is written:
  `{{ page.description or config.description }}`.

Truth: `null`, `false`, `0`, `""`, empty html and an empty list are false;
a date, an object and every other value are true.

## 4. Values and data

Types: null, bool, number (an integer), string, html (a string press
rendered from Markdown, inserted without escaping; nothing in the language
makes html from a string), date (a calendar day), list, map, and the
objects below. Front matter and config values map: string to string,
integer to number, boolean to bool, date and datetime to date, array to
list, table to map; a float is an error naming the file.

**Objects and maps.** Objects - `config`, `site`, page, section, term, year
group - have a fixed set of fields; reading any other name is an error
(`theme/post.html:12:8: page has no field "titel"`). Maps - `extra`, every
table inside it, and `site.sections` - hold arbitrary keys; a missing key
is `null` in `extra` and its tables (that is where optional front matter
lives) and an error in `site.sections`. `.name` on `null` or on anything
that is not an object or map is an error, so `page.extra.a.b` with `a`
absent reports `null has no field "b"`; write `page.extra.a and
page.extra.a.b`.

**Inserting.** `{{ }}` writes a string escaped (`& < > " '`), html as it
is, a number as digits, a date as `YYYY-MM-DD`. Inserting `null`, a bool,
a list, a map or an object is an error naming the expression
(`config.extra.gihub is null`); nothing renders as empty by accident.

Globals, in every template, partial and base:

| name | |
|---|---|
| `config` | `base_url`, `title`, `description`, `default_language`, `extra` (map) |
| `site` | `sections` (map name -> section), `projects` (list of the project sections, by weight then name), `posts` (list of every page inside a section, newest first), `years` (year groups of `posts`), `tags` (list of term, by name) |
| `scripts` | names of client-side libraries the rendered content needs (`mermaid`, `wavedrom`), a list; empty when there is no content |
| `current_path` | the root-relative URL of the page being rendered, e.g. `/writing/` |
| `year` | the current year, a number |

Per page kind, one of these is set (the others are `null`):

| template | variable |
|---|---|
| `index.html` (the root section), `section.html` (every other section) | `section` |
| `post.html` (a page inside a section), `page.html` (a page at the root) | `page` |

Front matter overrides the defaults: `template = "x.html"` on any file, and
`page_template = "x.html"` on an `info.md` for the pages of its section.
| `tags-list.html` | none beyond the globals (`site.tags`) |
| `tags-single.html` | `term` |

The content tree is the model: a directory with an `info.md` is a
section, its other `.md` files are its pages, `.md` files at the root are
pages of the root section (whose `info.md` is the home). A section is a
project unless its `info.md` says `project = false`.

A **page** has `title`, `date` (date or `null`), `url` (root-relative,
`/swd-protocol/`), `file` (its path under `content/`,
`stm32-bare-metal/02-swd-protocol.md`), `content` (html), `description`
(string or `null`), `tags` (list of term), `extra` (map), `section` (its
section, or `null` at the root), `project` (its section when that is a
project, else `null`), `prev` and `next` (page or `null`: its neighbours in
its section's `pages`, so in a project the previous and next entry of the
build log). A **section** has `name`,
`title`, `description` (string or `null`), `url`, `content` (html),
`project` (bool), `extra` (map), `pages` (in file-name order, so `01-`,
`02-` is the order; by date newest first or by `weight` when its `info.md`
says `sort_by = "date"` or `"weight"`), `years` (year groups of its dated
pages, newest year first). A **term** has `name`, `url`, `pages` (newest
first). A **year group** has `year` (a number) and `pages`.

## 5. Functions

A function given `null`, the wrong type or the wrong count of arguments is
an error naming the function and the argument.

| function | |
|---|---|
| `len(list)` | number of items |
| `take(list, n)` | the first n items, or all of them when the list is shorter; negative n is an error |
| `last(list)` | the last item, or `null` for an empty list |
| `has(list, x)` | whether the list contains x (`==`) |
| `lower(s)` | |
| `starts(s, prefix)` | |
| `pad(n, width)` | digits, zero-padded on the left |
| `plural(n, word)` | `word` when n is exactly 1, else `word` + `s` |
| `date(d, fmt)` | `fmt` uses `%Y`, `%m`, `%d`, `%b` (Jan), `%B` (January) |
| `url(path)` | `@/writing/x.md` names a content file and gives its page's URL (an error if there is no such file); any other path is a root-relative path under the output, unchecked: `main.css`, `atom.xml`, `tags/`, `fonts/x.woff2` |

## 6. Scope

The globals are the outermost scope; the page template's file, each `if`
branch, each `for` iteration, and each partial (whose parent is the
globals) is a scope. `for` variables and include arguments may shadow any
visible name for their scope (`for page in section.pages`, a partial
argument named `page`); `let` may not redeclare any visible name. Functions have
their own namespace: `let date = page.date` does not hide `date()`. The
keywords `if else end for in let include extend yield and or not true
false` cannot be names.

## 7. Errors

Every error is `theme/file.html:line:col: message`, the location being the
first byte of the token at fault (the field name in `page.titel`, the
function name in `len(3)`, the opener of an unclosed tag), line and column
1-based, column in bytes, path relative to the site root. An error inside
a partial or the body of an extending template carries one frame per
enclosing include or extend, innermost first:

    theme/_post_row.html:3:22: undefined variable "index"
      in include "_post_row.html" (theme/index.html:40:3)

Undefined variable, unknown function, missing field, null where a value is
inserted or passed, wrong argument, an unclosed tag or statement, a
mismatched `end`, an `include` of a file that does not exist, and the
`extend`/`yield` rules above are all errors.

## 8. Not in the language

Filters and pipelines; macros with bodies; named blocks and multi-level
inheritance (the base takes values as arguments and has one slot);
whitespace-control markers; `with`; `while`; assignment to fields or
reassignment of names; function definitions; list literals and indexing
(`last`, `take` and host fields cover the theme); arithmetic beyond `+`,
floats, ordering comparisons; map iteration; `upper`; `{{! }}` (the html
type replaces it); anything that could be a field on the object that owns
the data. The rule for the next request is the lace rule: if the host can
compute it, the template does not.
