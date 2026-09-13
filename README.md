# press

A static site generator, and the small languages it is made of. Rust, one
binary, a handful of dependencies.

    cargo install --path crates/press --locked           # press on PATH
    press -site ../my-site                               # build to <site>/public-press
    press -site ../my-site serve                         # build, serve, rebuild on change,
                                                         # reload open pages
    press -site ../my-site present talk.md -out talk.html  # one Markdown file as one
                                                         # self-contained HTML talk
    press -site ../my-site present talk.md               # the talk served, rebuilt on change
    cargo run --release -p lace -- styles.scss -o styles.css   # the CSS language on its own

Flags: `-site DIR` (default `.`), `-out DIR` (for `present`, the HTML file), `-templates DIR` (default
`<site>/theme`), `-cache DIR` (default `<site>/.press-cache`), `-port N`
(default 1112).

## Parts

| | | |
|---|---|---|
| `crates/lace` | CSS plus computation | [SPEC.md](crates/lace/SPEC.md) |
| `crates/press` | the generator, and its template language | [docs/templates.md](docs/templates.md) |

A site is `config.toml`, `content/` (a directory with an `info.md` is a
section - a project - and the other `.md` files in it are its posts, in
file-name order; `.md` files at the root are pages; `+++` TOML front
matter throughout), `theme/*.html` (the theme belongs to the site, not to
press), `sass/main.scss` (lace) and `static/`. Fenced code blocks whose info word names a renderer
draw diagrams: `mermaid` and `wave` in the browser, `bytefield` at build time
(`npx bytefield-svg`), `pair` for one diagram two ways (the fence's Mermaid
beside Graphviz's build-time SVG), `note` for callouts; build-time renders
are cached by content. Any other info word is a language for syntect.

`present` renders one Markdown file from anywhere on disk with the site's
Markdown, theme and stylesheet and the theme's `talk.html`, then folds every
stylesheet, font, script and image the page loads into the page itself. The
file opens offline and fetches nothing, so a talk that cannot be published
can still be given: F5 presents it, as on a post with `present = true`.
Root-relative references come from the site's `static/` (and the compiled
`main.css`), relative ones from beside the talk; any left pointing at a
server are listed as a warning.

## Design

Two languages, one rule each. lace is CSS plus variables, functions, mixins
and loops; anything CSS already does (nesting, `&`, `calc()`, `min()`)
passes through untouched, so a plain CSS file compiles to itself. The
template language is HTML plus insert, repeat, choose and compose; values
are computed by the generator and handed over, so there are no filters, no
macros and no inheritance chains. Both fail loudly: a typo is an error with
a file, line and column, never an empty string in the output.

The Markdown pipeline is pulldown-cmark with the site's conventions applied
in the event stream: fence dispatch, stable heading ids, GFM
autolinks. Highlighting is syntect with class-based output; the theme's
`syntax.css` is generated. The build is about 240 ms for a 24-page site;
serve polls the source tree every 300 ms and pushes a reload over an event
stream.

## Tests

    cargo test

lace: 204 golden cases written from the spec, the site stylesheet as a
fixture, a mutation fuzzer. press: unit tests. The parity test against the
previous generator's output served its purpose during the rewrite and is
gone; the site's own build is the acceptance test now.
