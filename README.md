# press

A static site generator, and the small languages it is made of. Rust, one
binary, a handful of dependencies.

    cargo install --path crates/press                    # press on PATH
    press -site ../my-site                               # build to <site>/public-press
    press -site ../my-site serve                         # build, serve, rebuild on change,
                                                         # reload open pages
    cargo run --release -p lace -- styles.scss -o styles.css   # the CSS language on its own

Flags: `-site DIR` (default `.`), `-out DIR`, `-templates DIR` (default
`<site>/theme`), `-cache DIR` (default `<site>/.press-cache`), `-port N`
(default 1112).

## Parts

| | | |
|---|---|---|
| `crates/lace` | CSS plus computation | [SPEC.md](crates/lace/SPEC.md) |
| `crates/press` | the generator, and its template language | [docs/templates.md](docs/templates.md) |

A site is `config.toml`, `content/*.md` with `+++` TOML front matter,
`theme/*.html` (the theme belongs to the site, not to press), `sass/main.scss`
(lace) and `static/`. Fenced code blocks whose info word names a renderer
draw diagrams: `mermaid` and `wave` in the browser, `bytefield` at build time
(`npx bytefield-svg`), `pair` for one diagram two ways (the fence's Mermaid
beside Graphviz's build-time SVG), `note` for callouts; build-time renders
are cached by content. Any other info word is a language for syntect.

## Design

Two languages, one rule each. lace is CSS plus variables, functions, mixins
and loops; anything CSS already does (nesting, `&`, `calc()`, `min()`)
passes through untouched, so a plain CSS file compiles to itself. The
template language is HTML plus insert, repeat, choose and compose; values
are computed by the generator and handed over, so there are no filters, no
macros and no inheritance chains. Both fail loudly: a typo is an error with
a file, line and column, never an empty string in the output.

The Markdown pipeline is pulldown-cmark with the site's conventions applied
in the event stream: fence dispatch, goldmark-compatible heading ids, GFM
autolinks. Highlighting is syntect with class-based output; the theme's
`syntax.css` is generated. The build is about 240 ms for a 24-page site;
serve polls the source tree every 300 ms and pushes a reload over an event
stream.

## Tests

    cargo test

lace: 204 golden cases written from the spec, the site stylesheet as a
fixture, a mutation fuzzer. press: unit tests and `tests/site.rs`, which
builds the real site with its own theme and compares it with the last build
of the previous generator, kept in the site as `public-go` (skipped when the
site is not checked out next to this repository).
