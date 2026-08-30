# How the Go press behaves

This is the behavioural specification of the Go static site generator
`press` (F:/github/press, ~1,900 lines, Go 1.27) as it builds
F:/github/jseketa.github.io into F:/github/jseketa.github.io/public-press.
It is written so the Rust rewrite can be implemented without re-reading the
Go. Every rule carries a file:line reference into the Go source, the
libraries it uses, or the reference output. Nothing here is inferred from a
file name; every claim was checked against the code or the built tree.

Source packages: cmd/press/main.go (entry), internal/config, internal/content,
internal/markdown, internal/blocks, internal/theme, internal/site.
Pinned libraries (F:/github/press/go.mod): github.com/BurntSushi/toml v1.6.0,
github.com/yuin/goldmark v1.8.5, github.com/alecthomas/chroma/v2 v2.27.0,
github.com/jseketa/lace (replace => ../lace), github.com/dlclark/regexp2/v2
v2.7.1 (indirect, used by chroma). Library sources live under
C:/Users/Bunny/go/pkg/mod/; the Go standard library under C:/Core/Go/src/.

Path conventions in this document: `press/` = F:/github/press,
`site/` = F:/github/jseketa.github.io, `out/` = site/public-press,
`theme/` = site/theme, `content/` = site/content. "The site" means
jseketa.github.io as it is today.

Reference build facts used throughout: 24 HTML pages (10 pages, 3 sections,
10 tag pages, 1 tag index), 71 output files, atom.xml is the only file that
differs between two builds of the same sources (its top-level `<updated>` is
the build time).

## Where the four readers disagreed, and what the Go actually does

1. Line endings of content files. Reader 1 stated every content file is CRLF
   and that the TOML block therefore ends in a stray CR. Reader 2 stated the
   files contain no CR bytes. Checked with `tr -cd '\r' | wc -c` over every
   content/*.md and a hex dump of content/_index.md: zero CR bytes in all 13
   files, and zero in the theme files and in the output. Reader 2 is right.
   The confusion comes from `git config core.autocrlf = true` in the site
   repository: another checkout could well produce CRLF, and the splitter
   tolerates that (section 2.2), but the reference build was made from LF
   files. Treat "input is LF" as the tested case and "input is CRLF" as
   supported-but-unexercised.

2. Trailing empty line in highlighted code. Reader 4 said chroma emits one
   `<span class="line">` per line "including a final empty line". Reader 2
   said the trailing empty line is dropped. chroma iterator.go:85-89 strips a
   last line that consists of a single empty token, and the reference output
   ends every block with `...last line\n</span></span></code></pre>` with no
   empty line element (out/blinking-a-led/index.html:345-350). Reader 2 is
   right: the final "\n" stays inside the last line's last token and no empty
   line follows it.

3. Trailing newline of rendered pages. Reader 3 said base.html ends with
   `</html>` and no newline, so pages end without one. A byte dump of
   theme/base.html and out/index.html shows both end in `</html>\n`. The
   template file ends with a newline and press writes the template output
   verbatim, so every page ends with "\n". Nothing is appended or stripped by
   press itself (theme.go:102-112, build.go:273-284).

4. Tabs inside fence bodies. Reader 2 said "tab/indent padding expanded to
   spaces"; reader 4 said "fence bodies keep tabs verbatim". Both are right
   about different things: goldmark's `Segment.Value` (text/segment.go:57-70)
   prepends `Padding` spaces only when a tab straddled the fence-indent
   removal boundary (an indented fence). For the site's fences, none of which
   is indented, every body line is byte-for-byte the source line, tabs
   included (out/blinking-a-led/index.html:285 shows a literal tab).

5. Everything else in the four reports agrees with the source. Where a
   reader's line numbers are cited below they were spot-checked against the
   files and found accurate.

## 1. Config

### 1.1 Command line

press/cmd/press/main.go:21-51.

| flag | default | note |
|---|---|---|
| `-site` | `.` | made absolute with `filepath.Abs` (main.go:28) |
| `-out` | `<site>/public-press` | empty string means default (main.go:38, `or()` at 54-59) |
| `-templates` | `<site>/theme` | main.go:39 |
| `-cache` | `<site>/.press-cache` | main.go:40 |
| `-port` | `1112` | int, serve only (main.go:25,44) |

- `log.SetFlags(0)`: fatal messages have no timestamp (main.go:19).
- If `<site>/config.toml` does not exist (`os.Stat` fails): fatal
  `no config.toml in <abs site>` (main.go:32-34).
- If the first positional argument is exactly `serve`: run
  `site.Serve(opts, "127.0.0.1:<port>", os.Stdout)`; the returned error is
  fatal (main.go:43-45). Loopback only.
- Otherwise one build. Failure: `build failed: <err>` and exit 1
  (main.go:47-50). Success prints
  `%d pages -> %s in %v\n` with `stats.Pages`, `opts.Out`,
  `stats.Duration.Round(1e6)` (rounded to 1 ms, printed with Go's
  `time.Duration.String`, e.g. `18.131s`, `152ms`, `1m2.5s`) (main.go:51).
  Observed: `24 pages -> <out> in 18.131s`.
- `Stats.Pages = len(site.Pages) + len(site.Sections) + len(site.Terms) + 1`;
  the `+1` is the tag index (build.go:91). For the site: 10 + 3 + 10 + 1 = 24.

### 1.2 config.toml

press/internal/config/config.go:6-33,59-65. Decoded with BurntSushi/toml
v1.6.0 `toml.DecodeFile` into this struct (toml tags are the key names):

```
Config {
  BaseURL         string     `base_url`
  Title           string     `title`
  Description     string     `description`
  DefaultLanguage string     `default_language`
  GenerateFeeds   bool       `generate_feeds`
  Taxonomies      []Taxonomy `taxonomies`      // Taxonomy{Name `name`, Feed bool `feed`}
  Markdown        Markdown   `markdown`        // {SmartPunctuation bool `smart_punctuation`, Highlighting {Style `style`, Theme `theme`}}
  Extra           Extra      `extra`           // map[string]any
}
```

Decoder semantics (BurntSushi decode.go):

- Every missing key is its Go zero value. There are no non-zero defaults and
  no required keys; a missing key never errors.
- Unknown keys are silently ignored. The site's `build_search_index`,
  `feed_filenames`, `render_emoji` are dropped this way.
- Key matching: exact struct-tag match first, then a case-insensitive
  fallback (`strings.EqualFold`), so `Title = ...` would also work.
- A value of the wrong TOML type for a typed field is a decode error and the
  build fails with `config: <err>` (build.go:58-60).

Which keys are consumed, and where:

| key | consumed by |
|---|---|
| `base_url` | feed only: one trailing `/` trimmed (`TrimSuffix`), then `/atom.xml`, `/`, and page URLs appended raw (build.go:230-245) |
| `title` | feed `<title>` (HTML-escaped) and templates `.Config.Title` |
| `description` | templates only (`.Config.Description`, base.html:8 default and page fallbacks) |
| `default_language` | templates only (base.html:2 `<html lang>`) |
| `generate_feeds` | gates atom.xml (build.go:223) |
| `markdown.smart_punctuation` | enables goldmark Typographer (build.go:69, markdown.go:71-73) |
| `markdown.highlighting.theme` | chroma style name (build.go:68, markdown.go:49) |
| `extra.*` | templates via `str`/`list`/`flag` (theme.go:132-134) |
| `taxonomies` | NEVER read. `tags` is hard-coded (content.go:111,178-190) |
| `markdown.highlighting.style` | NEVER read (declared config.go:19 only) |

### 1.3 Extra accessors

config.go:35-57. `Extra` is `map[string]any` straight from the TOML decoder.

- `Str(k)`: the value if it is a Go `string`, else `""` (missing key, nil map,
  wrong type all give `""`; `year = 2020` as an integer gives `""`).
- `Bool(k)`: the value if it is a `bool`, else `false`.
- `List(k)`: if the value is a TOML array (`[]any`), the string elements only,
  in order, non-strings silently dropped (`stack = ["C", 1]` -> `["C"]`); if
  the value is not an array or is missing, `nil`.

### 1.4 The site's config.toml (site/config.toml:1-29)

```
base_url = "https://seketa.it"
title = "Josip Seketa"
description = "Build logs from a backend developer - Node.js, Go, microservices, and the occasional microcontroller."
default_language = "en"
build_search_index = false          # ignored
generate_feeds = true
feed_filenames = ["atom.xml"]       # ignored
taxonomies = [ { name = "tags", feed = true } ]   # ignored
[markdown]
smart_punctuation = true
render_emoji = false                # ignored
[markdown.highlighting]
style = "class"                     # ignored
theme = "github-dark"
[extra]
tagline = "Backend developer"
headline = "Notes from the workbench"
github = "jseketa"
linkedin = "jseketa"
email = "jseketa@gmail.com"
```

## 2. Content model and URLs

### 2.1 Discovery

press/internal/content/content.go:76-127.

- `filepath.Walk(<site>/content)`: recursive; directory entries are visited
  in lexical (byte-wise) order, so `_` (0x5F) sorts before lowercase letters
  and uppercase before lowercase. Hidden directories are NOT skipped.
- Only files whose full path ends with `.md` (case-sensitive; `.MD` is
  ignored). Every other file is ignored and NOT copied anywhere.
- A missing content dir or an unreadable file aborts the build with
  `content: <err>` (content.go:80,88; build.go:62-64).
- `rel` = path relative to `content/` with forward slashes; this is
  `Page.Source` (e.g. `writing/swd-protocol.md`).
- A file whose `rel` ENDS with `_index.md` (suffix test, so `foo_index.md`
  also matches) is a Section with `Name = path.Dir(rel)` (`.` -> `""`). A
  later matching file for the same directory overwrites the earlier entry
  (content.go:96-107).
- Every other `.md` is a Page appended to `site.Pages` in walk order
  (content.go:109-118).
- No draft flag, no future-date filtering, no expiry: every `.md` is
  published.

Walk order for this site: `_index.md`, `about.md`, `projects/_index.md`,
`projects/learning-go.md`, `projects/ogame-scraper.md`,
`projects/stm32-bare-metal.md`, `writing/_index.md`,
`writing/blinking-a-led.md`, `writing/diagrams.md`,
`writing/golang-notes.md`, `writing/initial-design.md`,
`writing/microservices.md`, `writing/swd-protocol.md`.

### 2.2 Front matter block extraction (exact algorithm)

press/internal/content/frontmatter.go:27-44.

1. `s` = file bytes as a string with a leading UTF-8 BOM (EF BB BF) removed.
2. `s = TrimLeft(s, " \t\r\n")`.
3. If `s` does not start with `+++`: no front matter, all fields zero, body =
   `s` (the trimmed string), no error.
4. `rest = s[3:]`.
5. `end` = index of the first `"\n+++"` in `rest` (LF immediately followed by
   three pluses). Not found: error `unterminated +++ front matter`, wrapped as
   `<rel>: unterminated +++ front matter` and then `content: ...`; the build
   fails.
6. TOML text = `rest[:end]`. The remainder of the opening `+++` line is part
   of the TOML text. With CRLF input the TOML text ends with a stray `\r`,
   which BurntSushi accepts.
7. body = `TrimLeft(rest[end+4:], "\r\n")`. Spaces are NOT trimmed. Anything
   after the closing `+++` on the same line stays in the body. The closing
   fence only needs to start with `+++` at line start; the rest of that line
   is not checked. A TOML multi-line string containing a line that starts
   with `+++` terminates the block early.

### 2.3 Front matter fields

frontmatter.go:14-25. All optional, zero defaults, TOML types enforced by
BurntSushi (wrong type -> decode error -> build fails as `content: <rel>: ...`).

```
frontMatter {
  Title        string              `title`
  Description  string              `description`
  Date         time.Time           `date`            // see 2.4
  Weight       int                 `weight`          // TOML integer only; 1.0 or "1" fail
  Path         string              `path`            // URL override, see 2.7
  SortBy       string              `sort_by`         // "weight" | "date" | anything else = unsorted
  Template     string              `template`        // sections only; parsed and ignored on pages
  PageTemplate string              `page_template`   // sections only
  Taxonomies   map[string][]string `taxonomies`      // only taxonomies.tags is read
  Extra        config.Extra        `extra`           // nil when absent
}
```

- Unknown keys are silently ignored (`generate_feeds = true` in
  content/writing/_index.md:6 has no effect).
- Key matching is exact, then case-insensitive (same decoder as config).
- For pages a nil `Extra` is replaced by an empty map (content.go:114-116);
  for sections it stays nil.
- `Title`, `Description`, `Tags` are used verbatim: no trimming, no
  normalisation.

### 2.4 Date parsing: a TOML local date gets the BUILD MACHINE's offset

BurntSushi toml@v1.6.0 parse.go:345-379 tries these layouts in order with
`time.ParseInLocation`:

| layout | location |
|---|---|
| `time.RFC3339Nano` | `time.Local` |
| `2006-01-02T15:04:05.999999999` | `internal.LocalDatetime` |
| `2006-01-02` | `internal.LocalDate` |
| `15:04:05.999999999` | `internal.LocalTime` |
| `2006-01-02T15:04Z07:00` | `time.Local` |
| `2006-01-02T15:04` | `internal.LocalDatetime` |
| `15:04` | `internal.LocalTime` |

internal/tz.go:31-36: `localOffset = time.Now().Zone()` offset computed ONCE
at process start; `LocalDate = time.FixedZone("date-local", localOffset)`.
So `date = 2018-06-02` becomes midnight with the UTC offset in force on the
machine WHEN THE BUILD RUNS (the build day's DST, not the date's). Values
with a missing leading zero are rejected (`missingLeadingZero`). The value is
then delivered into the `time.Time` field (round-tripped through
MarshalText/UnmarshalText in decode.go), keeping the wall clock and the fixed
offset.

Consequences:
- Reference build ran at UTC+02:00, so `2026-08-22` ->
  `2026-08-22T00:00:00+02:00`, whose UTC form is `2026-08-21T22:00:00Z`; that
  is exactly what out/atom.xml:12 contains. Built in winter (CET) the feed
  would say `T23:00:00Z`.
- Templates format the time in its own zone (`t.Format`), so HTML pages show
  the calendar date as written and are unaffected.
- A quoted string `date = "2018-06-02"` FAILS the build (time.Time's
  UnmarshalText needs RFC3339); `date = "2018-06-02T00:00:00Z"` works.
- Missing date = the zero time `0001-01-01 00:00:00 UTC`.

### 2.5 Data model

content.go:18-74.

```
Page {
  Title, Description string
  Date        time.Time        // 2.4
  Weight      int
  Tags        []string         // = fm.Taxonomies["tags"], nil if absent, order as written
  Extra       config.Extra     // never nil for pages
  Body        string           // raw markdown after front matter
  Content     template.HTML    // rendered body (set by build.render)
  Scripts     []string         // client libs the rendered blocks need ("mermaid", "wavedrom")
  URL         string           // root-relative, leading and trailing "/"
  Section     *Section         // nil if the directory has no _index.md
  Earlier     *Page            // older; set only in sort_by = "date" sections
  Later       *Page            // newer; same
  Source      string           // rel path, forward slashes
}
func (p *Page) NeedsScript(name string) bool   // linear search of Scripts (content.go:38-45)

Section {
  Name  string                 // "" root, "projects", "writing"; nested dir -> "a/b"
  Title, Description, SortBy, Template, PageTemplate string
  Body  string
  Content template.HTML
  Pages []*Page                // sorted per SortBy
  URL   string
  Extra config.Extra           // may be nil
}

Term  { Name string /* raw tag */; URL string /* /tags/<slug>/ */; Pages []*Page /* date desc */ }
func (t *Term) PageCount() int  // len(Pages) (content.go:67)

Site  { Root string /* abs site dir */; Sections map[string]*Section; Pages []*Page /* walk order */; Terms []*Term /* Name-sorted */ }
```

### 2.6 Section attachment and sorting

content.go:146-176.

- After the walk each page is attached to the section whose `Name ==
  path.Dir(page.Source)` (`.` -> `""`): `page.Section = sec` and the page is
  appended to `sec.Pages` in walk order. Only the immediate directory counts:
  a page in `a/b/` attaches to section `a/b` only, never to `a`. A page whose
  directory has no `_index.md` gets `Section = nil` (still rendered, with
  page.html; listed nowhere). `about.md` attaches to the root section `""`
  because `content/_index.md` exists.
- Sorting, per section (`sort.SliceStable`):
  - `sort_by = "weight"`: ascending by `Weight`; ties keep walk order.
  - `sort_by = "date"`: newest first (`pages[i].Date.After(pages[j].Date)`);
    equal dates keep walk order; zero dates sink to the end. Then for index
    `i`: `Earlier = pages[i+1]` (older), `Later = pages[i-1]` (newer); the
    first has `Later = nil`, the last `Earlier = nil` (content.go:163-172).
  - anything else (including `""`): walk order, no Earlier/Later.
- Section map iteration (render, writeSections) is Go map order (random) but
  output-independent.

Resulting order for the site: projects (weight): learning-go 1,
ogame-scraper 2, stm32-bare-metal 3. writing (date desc): diagrams
2026-08-22, swd-protocol 2026-08-21, golang-notes 2022-10-16, microservices
2020-09-27, initial-design 2020-08-23, blinking-a-led 2018-06-02. Root
section: `[about]`, unsorted, no Earlier/Later.

### 2.7 URL derivation

content.go:129-144 (`pageURL`).

- If `path` is set: `"/" + strings.Trim(path, "/") + "/"` (`about` ->
  `/about/`, `/a/b/` -> `/a/b/`). Applies to sections too. No normalisation
  beyond trimming slashes: `path = "About"` gives `/About/`; backslashes kept.
- Else strip `.md` from `rel`:
  - `rel == "_index"` -> `/`
  - `rel` ends with `/_index` -> `"/" + dir + "/"` (`writing/_index` -> `/writing/`)
  - otherwise `"/" + rel + "/"` (`projects/learning-go` -> `/projects/learning-go/`)
- Filenames are used verbatim: no slugifying, no lowercasing, spaces kept.
- Every URL is root-relative and starts and ends with `/`.
- Term URL: `"/tags/" + Slugify(name) + "/"`; tag index: `/tags/`.

The site's URLs: `/`, `/about/` (path override), `/projects/`,
`/projects/learning-go/`, `/projects/ogame-scraper/`,
`/projects/stm32-bare-metal/`, `/writing/`; posts all via path override:
`/blinking-a-led/`, `/diagrams/`, `/golang-notes/`, `/initial-design/`,
`/microservices/`, `/swd-protocol/`; `/tags/`, `/tags/c/`, `/tags/embedded/`,
`/tags/golang/`, `/tags/meta/`, `/tags/microservices/`, `/tags/nodejs/`,
`/tags/scraping/`, `/tags/stm32/`, `/tags/swd/`, `/tags/tooling/`.

### 2.8 Internal link resolution (`@/` references)

content.go:230-245 (`Site.Resolve(ref) (string, bool)`).

1. `rel = TrimPrefix(ref, "@/")`.
2. If `rel` ends with `_index.md`: `name = TrimSuffix(TrimSuffix(rel,
   "_index.md"), "/")` (`@/_index.md` -> `""`, `@/projects/_index.md` ->
   `projects`); if that section exists return its URL.
3. Otherwise (or if the section is missing) scan `site.Pages` in walk order
   for `Source == rel`; first match wins; return its URL.
4. Miss: `("", false)`; the template `url` function then emits `/`.

Resolve is only reached through the template `url` function; the site's
markdown contains no `@/` links.

### 2.9 Slugify (tag URLs only)

content.go:247-263.

- Lowercase the whole string with Unicode `strings.ToLower`.
- For each rune: ASCII `a-z` and `0-9` are kept; ANY other rune (including
  non-ASCII letters such as U+00E9, which are dropped, not transliterated)
  emits a single `-` but only if the output is non-empty and the previous
  emitted character was not already `-` (runs collapse, never a leading dash).
- Finally `strings.Trim(result, "-")`.
- Examples: `Node.js` -> `node-js`, `C++` -> `c`, `  Foo  Bar ` -> `foo-bar`,
  a string of only non-ASCII letters -> `""`.
- `Term.Name` keeps the raw tag string for display; only the URL is slugified.

### 2.10 Taxonomies (tags only)

content.go:111,178-190; build.go:157-179.

- `Page.Tags = fm.Taxonomies["tags"]` in the order written (nil if absent).
  Every other taxonomy name is ignored.
- `buildTerms`: for every page in walk order, for every tag string
  (case-sensitive, untrimmed), append the page to `byName[tag]` (a tag
  repeated inside one page adds the page twice). Each `Term{Name: raw tag,
  URL: /tags/<slug>/, Pages: stable-sorted newest first}`. The Terms list is
  sorted by `Name` ascending, byte-wise (`Z` < `a`).
- Tagged pages of ANY section (or none) are included, not just writing.
- `/tags/` is ALWAYS written (even with zero terms) via `tags-list.html`
  with `Ctx.Terms` and `CurrentPath = "/tags/"`; each term via
  `tags-single.html` with `Ctx.Term` and `CurrentPath = term.URL`, file
  `tags/<slug>/index.html`.
- The site's terms and counts: c 1, embedded 2, golang 1, meta 1,
  microservices 1, nodejs 2, scraping 1, stm32 2, swd 1, tooling 1.

### 2.11 Project <-> post linking (extra.project / extra.slug)

content.go:192-228.

- `PostsFor(slug)`: nil if `slug == ""` or no section named `writing`; else
  every page of the writing section, in section order (date desc), whose
  `Extra.Str("project") == slug`.
- `OwnerOf(page)`: `slug = page.Extra.Str("project")`; nil if empty or no
  `projects` section; else the first projects-section page (weight order)
  whose `Extra.Str("slug") == slug`; nil on a miss, never an error.
- Exact string equality; nothing validates that a referenced slug exists.

Site data: learning-go <- golang-notes; ogame-scraper <- microservices,
initial-design; stm32-bare-metal <- swd-protocol, blinking-a-led; diagrams.md
has no `[extra]` at all, so `ownerOf` is nil for it.

Extra keys the theme reads: projects: `slug`, `year` (`"2022 - now"`,
`"2020"`, `"2018"`), `status` (`"Active"`, `"Archived"`), `stack` (string
arrays of 1-4 items), `repo` (read by project.html:11, set by no file);
posts: `project`.

### 2.12 Descriptions, summaries, drafts

There is no summary/excerpt extraction, no reading time, no word count and
no draft flag anywhere (grep over press/internal for
summary|reading|words|excerpt|draft finds nothing). `description` is the only
text metadata: used verbatim by templates for `<meta name="description">`
(falling back to the config description when empty) and as the project card
blurb. `Section.Description` is loaded but unused by this theme.
`Page.Content` is the full rendered body and is also the feed content.

## 3. Markdown pipeline (with exact HTML shapes)

### 3.1 Renderer construction

press/internal/markdown/markdown.go:22-81; build.go:35-44,67-72.

```
markdown.Options{ Theme: cfg.Markdown.Highlighting.Theme,      // "github-dark"
                  SmartPunctuation: cfg.Markdown.SmartPunctuation,  // true
                  Root: abs site dir, CacheDir: -cache or <site>/.press-cache }
```

- Chroma style: `styles.Get(theme)`; an unknown name silently yields
  `styles.Fallback` (`swapoff`); the nil check at markdown.go:50 is dead
  because `styles.Get` never returns nil.
- Chroma formatter: `chromahtml.New(chromahtml.WithClasses(true))` with no
  other options: class prefix `""`, no line numbers, no tab width, no
  wrapping, no mode classes.
- goldmark: `goldmark.New(WithExtensions(extension.GFM, extension.Footnote,
  &fenceExtension{}, [extension.Typographer only if SmartPunctuation]),
  WithParserOptions(parser.WithAutoHeadingID()),
  WithRendererOptions(html.WithUnsafe()))`.
  GFM = Linkify + Table + Strikethrough + TaskList (goldmark
  extension/gfm.go:13-18).
- NOT enabled: `WithAttribute` (so `{#id}` / `{.class}` suffixes are literal
  text), `WithHardWraps` (a soft line break renders as a bare `\n`; a hard
  break -- two trailing spaces or `\` at EOL -- renders `<br>\n`),
  `WithXHTML` (void tags are `<hr>`, `<img ...>`, `<br>`), definition lists,
  CJK, EastAsianLineBreaks.
- `WithUnsafe`: HTML blocks and inline raw HTML are copied through verbatim
  (NUL -> U+FFFD); `javascript:`, `data:`, `file:`, `vbscript:` URLs are NOT
  stripped from href/src.
- Registry (build.go:35-44): Mermaid `mermaid`, Wave `wave`, Note `note`,
  Graphviz `dot`, Bytefield `bytefield`, Pair `pair`.
- One `Renderer` serves all pages, serialised by a mutex.

### 3.2 Render entry point

markdown.go:83-101; build.go:94-110.

- `Render(src)`: lock; `scripts = map{}`; `md.Convert(src, &buf)`; return
  `Result{HTML: buf, Scripts: keys of the set}` (Go map order, i.e. random;
  only membership is ever used).
- Pages: `pg.Content = template.HTML(res.HTML)`, `pg.Scripts = res.Scripts`,
  in walk order; errors are `"<pg.Source>: <err>"`. Sections: rendered with
  the same `Render` in map order; `Scripts` discarded; errors `"section
  <name>: <err>"`. Any renderer error aborts the build.
- There is NO post-processing of the HTML anywhere: no table wrapping, no
  image/figure rewriting, no external-link rel/target, no heading anchors.
  Templates insert `{{.Page.Content}}` raw (`template.HTML`).
- `fragment(src)` (markdown.go:103-112) runs `md.Convert` with a new parser
  context WITHOUT taking the lock; it is only called from inside `Render`
  (Note bodies) and writes script discoveries into the same page set.

### 3.3 Core goldmark HTML shapes (renderer/html/html.go)

All of these were confirmed in the reference output.

- Text escaping (goldmark `util.EscapeHTML`, writer `Write`): `&` -> `&amp;`,
  `<` -> `&lt;`, `>` -> `&gt;`, `"` -> `&quot;`; `'` NOT escaped; NUL ->
  U+FFFD; backslash escapes and entity/numeric references are resolved in
  text. Note that this differs from chroma (`&#34;`, `&#39;`) and from
  html/template (`&#34;`, `&#39;`, `&#43;`).
- Paragraph: `<p>` inline `</p>\n`.
- Heading: `<hN id="ID">` inline `</hN>\n` (ids: 3.4).
- Blockquote: `<blockquote>\n` children `</blockquote>\n`.
- Thematic break: `<hr>\n`.
- Lists (html.go:408-454): `<ul>\n` or `<ol>\n` (`<ol start="N">\n` when the
  start number is not 1); item `<li>` then `\n` ONLY if the item's first
  child is not a TextBlock (i.e. loose items and items starting with a block),
  children, `</li>\n`; close `</ul>\n` / `</ol>\n`. A tight item renders its
  text without `<p>`; when a tight item has a nested list the text is
  followed by `\n` then the nested `<ul>\n...</ul>\n`, then `</li>\n`
  (out/microservices/index.html:94-106):
  ```
  <ul>
  <li>accepted arguments
  <ul>
  <li>name</li>
  <li>language</li>
  </ul>
  </li>
  ```
  A loose item is `<li>\n<p>..</p>\n</li>\n`. An ordered list may interrupt
  a paragraph without a blank line (`<p>..</p>\n<ol>\n<li>..</li>\n...`,
  out/initial-design/index.html:93-97).
- Emphasis `<em>..</em>`, strong `<strong>..</strong>`, strikethrough
  `<del>..</del>` (not used by the site).
- Code span: `<code>` escaped text `</code>`; a newline inside becomes a
  space.
- Link: `<a href="ESC(URLEscape(dest))"` + optional ` title="ESC(title)"` +
  `>` children `</a>`. `util.URLEscape`: space -> `%20`, non-ASCII bytes
  percent-encoded, existing `%XX` kept; then HTML-escape (& " < >).
- Image: `<img src="..." alt="<plain text of the children>"` + optional
  ` title="..."` + `>`; no self-closing slash, no loading/width attributes,
  no figure wrapping. Two consecutive image lines share one `<p>` with a
  `\n` between them:
  `<p><img src="/assets/images/boring_writer.gif" alt="Boring writer" title="Boring writer">\n<img src="/assets/images/concept_level.gif" alt="Concept level" title="Concept level"></p>`
  (out/initial-design/index.html:88-89).
- Raw HTML blocks and inline HTML: verbatim. The site's only raw HTML is
  `<!-- TODO(josip): ... -->` comments (content/_index.md:6, about.md:7-10,
  learning-go.md:15-16, golang-notes.md:38, swd-protocol.md:231), which pass
  through into the pages (and into the hero `<div class="hero__bio">` on `/`).
- Soft line break: `\n`. Hard line break: `<br>\n` (none in the site's
  content).
- Task list item: `<li><input checked="" disabled="" type="checkbox"> text`
  or `<li><input disabled="" type="checkbox"> text` (unused by the site).

### 3.4 Heading IDs (`parser.WithAutoHeadingID`)

goldmark parser/parser.go:99-142; parser/atx_heading.go:120-177;
parser/setext_headings.go:111-114.

- Input: the RAW source bytes of the heading's LAST line (for ATX: the text
  after the leading `#`s, with a closing run of `#`s and trailing whitespace
  removed). Markdown syntax is not stripped: `` ## Using `go build` `` ->
  `using-go-build`; `## [x](y)` -> `xy`.
- Algorithm: trim leading/trailing bytes in `" \t\n\x0b\x0c\x0d"`; iterate
  bytes: multi-byte UTF-8 sequences are skipped entirely (non-ASCII letters
  vanish); ASCII `[A-Za-z0-9]` kept, upper-cased to lower; ` `, `\t`, `\n`,
  `\r`, `-`, `_` each become `-` (NOT collapsed: `a - b` -> `a---b`);
  everything else is dropped (apostrophes, commas, backticks, brackets).
- Empty result -> `heading`.
- Uniqueness per `Convert` call: if already used try `<id>-1`, `<id>-2`, ...
- Rendered as `<h2 id="...">...</h2>\n`. Examples from the site:
  `What I'd do differently` -> `what-id-do-differently`;
  `SWO, the optional third wire` -> `swo-the-optional-third-wire`;
  `<h2 id="entry-to-the-main-program">` (out/blinking-a-led/index.html).
  The visible heading text still gets typographer substitution (`I&rsquo;d`).
- The site uses only `##` (29 h2) and `###` (3 h3, diagrams.md: Sequence,
  Gantt, Pie); h1 comes from templates. 32 headings in total, 31 distinct
  ids site-wide (`introduction` appears on two different pages, which is
  fine because uniqueness is per Convert); no `-N` suffix is exercised.

### 3.5 Typographer (smart punctuation), exact substitutions

goldmark extension/typographer.go:69-83,162-328,344-348. Inline parser at
priority 9999, triggered on `' " - . , < > * [`. Every substitution emits
HTML entity TEXT (not Unicode characters) as a `String` node with
`IsCode=true`, so it is written raw and must not be re-escaped.

- `---` -> `&mdash;` (checked first, needs 3 chars); `--` -> `&ndash;`;
  `...` -> `&hellip;`; `<<` -> `&laquo;`; `>>` -> `&raquo;`.
- Double quote (`"`): a delimiter scan decides can-open/can-close from the
  preceding character. Opening (`CanOpen && !CanClose`) -> `&ldquo;` and the
  per-block `Double` counter increments. Closing -> `&rdquo;` and the counter
  decrements, only when the counter > 0, where closing means
  `CanClose && !CanOpen`, or `CanClose && CanOpen` and the next char is
  punctuation followed by punctuation/space/end. Special case: `21""` (a
  digit before, a second `"` after) is left alone.
- Single quote (`'`): apostrophe rules first, giving `&rsquo;`: decades
  (`'90s`: two digits then `s`, followed by space/punct/end); `'t`, `'e`,
  `'n`, `'l` after space or punctuation (`'twas`, `'em`, `'net`); any `'`
  between a letter/digit and a letter. Then opening (`CanOpen &&
  !CanClose`) -> `&lsquo;` and `Single++`, EXCEPT `'s`, `'m`, `'t`, `'d`
  followed by punct/space/end and `'ve`, `'ll`, `'re` followed by
  punct/space/end, which give `&rsquo;` without counting. Then plural
  possessive / trailing (`Smiths'`, `doin'`) -> `&rsquo;`. Then closing when
  `Single > 0` (same isClose/maybeClose rule as double) -> `&rsquo;`,
  `Single--`.
- Unmatched quotes stay literal. Counters reset at the end of each block.
- Not applied inside code spans, code blocks, autolinks, or block-renderer
  bodies and captions.
- Site output counts: `&rsquo;` 26, `&ldquo;` 12, `&rdquo;` 12, `&mdash;` 16,
  `&hellip;` 12 (all inside blinking-a-led table cells); `&lsquo;`,
  `&ndash;`, `&laquo;`, `&raquo;` 0.

### 3.6 GFM: tables, strikethrough, task lists, linkify

- Tables (extension/table.go:382-540, paragraph transformer priority 200):
  ```
  <table>
  <thead>
  <tr>
  <th>..</th>
  ...
  </tr>
  </thead>
  <tbody>
  <tr>
  <td>..</td>
  </tr>
  ...
  </tbody>
  </table>
  ```
  `<tbody>` only if there is at least one body row; aligned columns get
  ` style="text-align:left|center|right"` on `th`/`td` (the non-XHTML default
  is the style method, table.go:522). The site's 9 tables (blinking-a-led.md
  124-126, 136-138, 144-149, 152-157; swd-protocol.md 28-34, 124-133,
  164-168, 209-214, 237-242) have no alignment colons; cells contain `...`,
  `>` (-> `&gt;`), inline code and bold.
- Strikethrough `~~x~~` -> `<del>x</del>`.
- Linkify (extension/linkify.go:14-16,158-296; priority 999; triggers after
  space, line start, `*`, `_`, `~`, `(`): bare URLs matching
  `^(?:http|https|ftp)://[-a-zA-Z0-9@:%._\+~#=]{1,256}\.[a-z]+(?::\d+)?(?:[/#?][-a-zA-Z0-9@:%_+.~#$!?&/=\(\);,'">\^{}\[\]`]*)?`,
  `www.` prefixes matching
  `^www\.[-a-zA-Z0-9@:%._\+~#=]{1,256}\.[a-z]+(?:[/#?][-a-zA-Z0-9@:%_\+.~#!?&/=\(\);,'">\^{}\[\]`]*)?`
  (href gets `http://` prepended, label unchanged), and bare emails (href
  `mailto:`). Trailing `.`, an unbalanced `)` and `;` entity tails are
  trimmed, as are trailing `? ! . , : * _ ~`. Rendered
  `<a href="URL">LABEL</a>` with URLEscape then HTML-escape. Not applied
  inside link labels. A bare domain without scheme (`apitester.com`) is NOT
  linked. Site example:
  `<a href="https://ozh.github.io/ascii-tables/">https://ozh.github.io/ascii-tables/</a>`.

### 3.7 Footnotes (`extension.Footnote`, defaults)

goldmark extension/footnote.go:312-321,528-625; out/initial-design/index.html:124,189-199.

- Reference `[^n]` -> `<sup id="fnref:N"><a href="#fn:N" class="footnote-ref" role="doc-noteref">N</a></sup>`
  where N is the 1-based order of first use; a second reference to the same
  note gets ids `fnref1:N`, `fnref2:N`, ...
- At the end of the document:
  ```
  <div class="footnotes" role="doc-endnotes">
  <hr>
  <ol>
  <li id="fn:N">
  <p>body ...&#160;<a href="#fnref:N" class="footnote-backref" role="doc-backlink">&#x21a9;&#xfe0e;</a></p>
  </li>
  ...
  </ol>
  </div>
  ```
  The backlink (one per reference) is appended inside the note's last
  paragraph preceded by `&#160;`. No id prefix, no titles. Unreferenced
  definitions are dropped. The `<div class="footnotes"` follows the previous
  block directly (after a fence it is `</code></pre><div class="footnotes"`,
  no newline, because the fence renderer writes no trailing newline).

### 3.8 Fenced code block dispatch

press/internal/markdown/fence.go:22-86. A node renderer registered at
priority 100 replaces goldmark's rendering of `KindFencedCodeBlock` and
`KindCodeBlock`.

- `info` = the fence's info string as goldmark stores it: leading and
  trailing whitespace trimmed (goldmark parser/fcode_block.go:50-63). A
  backtick fence whose info contains a backtick is not a fence at all.
- `name, attrs = blocks.ParseInfo(info)` (3.9).
- `body` = concatenation of the block's line segments (fence.go:78-86):
  each line exactly as written after fence-indent removal, every line ending
  in `\n` (`ForceNewline` adds one at EOF if missing), `Padding` spaces
  prepended only when a tab straddled the indent boundary. Tabs and trailing
  whitespace inside the body are kept verbatim.
- If the registry has `name`: `Renderer.Render(w, body, attrs, env)`; on
  error the build fails with `` ```<name>: <err> ``; after success, if the
  renderer implements `Script()` and returns non-empty, that string is added
  to the page's script set; children skipped.
- Otherwise `blocks.RenderCode(w, body, name, attrs, env)` with `name` as the
  chroma language (error propagated unwrapped).
- Indented (4-space / tab) code blocks: `RenderCode(w, body, "", Attr{}, env)`:
  always plain, never a renderer; goldmark strips trailing blank lines from
  indented blocks. The site has one (blinking-a-led.md:255-260, tab-indented,
  rendered as a plain chroma block with the tab stripped and `=>` as `=&gt;`).
- Neither path writes a trailing `\n` after the block (goldmark's own
  renderer would). Output is e.g. `</code></pre><p>` and
  `</code></pre></figure><p>` (out/golang-notes/index.html:103).

### 3.9 ParseInfo: the info string grammar

press/internal/blocks/blocks.go:107-163.

1. Skip leading spaces/tabs; `name` = run of non-space/tab characters.
2. Loop: skip spaces/tabs; at end -> return.
3. `key` = characters up to `=`, space or tab. Empty key (a stray `=`) ->
   return silently.
4. If the next char is not `=`: flag, `a[key] = key` (`wide` => `wide="wide"`).
5. Else consume `=`. If the next char is `"` or `'`: value = everything up to
   the matching same quote (spaces allowed); an unterminated quote takes the
   rest of the string; the closing quote is consumed. No escape sequences.
6. Else value = characters up to space/tab (may be empty: `k=` gives `""`).
7. Keys are case-sensitive; later duplicates overwrite.
- `Attr.Get(k, fallback)` returns `fallback` when the key is missing OR the
  value is empty. `Attr.Has(k)` is true only for a non-empty value
  (blocks.go:88-98).
- Consequence for the site: `` ``` file=linker.ld `` (blinking-a-led.md:168)
  and `` ``` file=diagrams/login.dot `` (diagrams.md:39) have a leading
  space, goldmark trims it, so `file=linker.ld` becomes the NAME: an unknown
  chroma language with no attributes -> plain `<pre class="chroma">` with no
  file bar and no token spans (out/blinking-a-led/index.html:284-297,
  out/diagrams/index.html:158-162). Likewise `` ``` json `` behaves as `json`.

Complete fence inventory of the site (file:line -> info string):
stm32-bare-metal.md:25 `note title="Learned the hard way"`;
blinking-a-led.md:42,68 `asm file=startup.s`; :85 `c file=programentry.c`;
:168 ` file=linker.ld`; :195,202,206,219,225,233,237,241,245 empty info (9
blocks); :265 `bat label="J-Link Commander"`; diagrams.md:30 `pair src=login
caption="..."`; :39 ` file=diagrams/login.dot`; :50 `pair src=swj
caption="..."`; :61 `pair src=services caption="..."`; :84 `pair
src=scraper-er caption="..."`; :101 `pair src=rebuild caption="..."`;
:120,133,150 `mermaid caption="..."`; golang-notes.md:26 `go file=hello.go`;
initial-design.md:48,59 ` json`; :67 `mermaid caption="..."`; :78
`javascript`; microservices.md:38 `json`; :70 `js`; swd-protocol.md:53 `wave
caption="..."`; :71,144 `c file=swd.c`; :106,185,203 `text`; :137 `bytefield
src=swd-request caption="..."` with an EMPTY body. Attribute keys used:
`file`, `label`, `caption`, `src`, `title`. Languages used: asm, c, bat, go,
json, javascript, js, text, and empty. Captions contain apostrophes
(`Mermaid's`, `Crow's`).

### 3.10 RenderCode: language fences and titled code figures

press/internal/blocks/prose.go:37-61.

- `html = env.Highlight(body, lang)` (section 5).
- If neither `file` nor `label` is non-empty: write `html` alone.
- Else:
  `<figure class="code"><figcaption class="code__bar"><span class="code__dots" aria-hidden="true"></span>`
  + (if file) `<span class="code__file">` + escape(file) + `</span>`
  + (if label) `<span class="code__label">` + escape(label) + `</span>`
  + `</figcaption>` + html + `</figure>`.
- Other attrs (caption etc.) are ignored for code.
- Observed (out/blinking-a-led/index.html:107):
  `<figure class="code"><figcaption class="code__bar"><span class="code__dots" aria-hidden="true"></span><span class="code__file">startup.s</span></figcaption><pre class="chroma">...`
  6 file bars (startup.s x2, programentry.c, hello.go, swd.c x2) and 1 label
  bar (J-Link Commander) in the site.

### 3.11 Nested fragment rendering (Note bodies)

markdown.go:103-112; fence.go:54-56; prose.go:19.

`env.Markdown = Renderer.fragment` runs `md.Convert` on the fragment with a
NEW parser context. Consequences: heading ids inside a note restart the
uniqueness table (can duplicate an id used elsewhere on the page); footnotes
inside a note form their own numbered list rendered inside the note;
typographer quote counters are independent; fences inside a note body
(including mermaid/wave) dispatch normally and record scripts on the
enclosing page. Nested fences need a longer/different fence marker to
survive the outer fence. The site's one note has no headings, footnotes or
fences.

## 4. Renderers and cache

### 4.1 Interfaces and environment

press/internal/blocks/blocks.go:24-83.

```
Env { Root string; CacheDir string;
      Highlight func(src []byte, lang string) (string, error);   // chroma
      Markdown  func(src []byte) (string, error) }               // full goldmark pipeline on a fragment
Renderer interface { Name() string; Render(w io.Writer, src []byte, a Attr, env *Env) error }
NeedsScript interface { Script() string }   // optional
Registry: map name -> Renderer; Get(name); Script(name) -> (Script(), true) if NeedsScript else ("", false)
```

Scripts: Mermaid -> `mermaid`, Wave -> `wavedrom`, Pair -> `mermaid`;
Note/Graphviz/Bytefield none. The theme tests `.Page.NeedsScript "mermaid"`
/ `"wavedrom"` to emit deferred `<script>` tags (base.html:86-93). Pages
needing scripts in the site: initial-design and diagrams -> mermaid;
swd-protocol -> wavedrom.

### 4.2 Figure wrapper and escape helper

blocks.go:165-190.

- `Figure(w, class, attrs, body)`: writes `<figure class="<class>">` (Go
  `%q`; classes are constant ASCII so no escaping happens), then the body,
  then if `attrs.caption` is non-empty `<figcaption>` + escape(caption) +
  `</figcaption>`, then `</figure>`. No newlines anywhere.
- `escape()` replaces only `&` -> `&amp;`, `<` -> `&lt;`, `>` -> `&gt;`,
  `"` -> `&quot;` (single quote untouched). Captions are NOT run through the
  typographer: `Mermaid's` stays `Mermaid's`, `"` becomes `&quot;`, `--`
  stays `--`.
- Observed: `<figcaption>The 8-bit request, least significant bit first.</figcaption></figure>`
  (out/swd-protocol/index.html:254).

### 4.3 mermaid (client-side)

blocks/client.go:10-23.
`<figure class="diagram"><pre class="mermaid">` + escape(body) + `</pre>` +
[figcaption] + `</figure>`. Body HTML-escaped so `<|--` survives as text and
`<br/>` becomes `&lt;br/&gt;`. Script `mermaid`. Observed:
`<figure class="diagram"><pre class="mermaid">sequenceDiagram` (out/diagrams/index.html:415).

### 4.4 wave (client-side WaveDrom)

blocks/client.go:30-42.
`<figure class="diagram diagram--wave"><script type="WaveDrom">` + body RAW
(no escaping) + `</script>` + [figcaption] + `</figure>`. Script `wavedrom`.
Observed: `<figure class="diagram diagram--wave"><script type="WaveDrom">{ "signal": [`
(out/swd-protocol/index.html:146).

### 4.5 note (prose callout)

blocks/prose.go:14-28.
Body rendered as markdown via `env.Markdown` (full pipeline, fresh parser
context), then
`<aside class="note note--` + escape(kind or `info`) + `">` +
`<p class="note__label">` + escape(title or `Note`) + `</p>` +
`<div class="note__body">` + fragmentHTML + `</div></aside>`.
The fragment normally ends with `\n` (goldmark ends `</p>\n`). No script.
Errors from the nested render propagate. Observed
(out/projects/stm32-bare-metal/index.html:97):
`<aside class="note note--info"><p class="note__label">Learned the hard way</p><div class="note__body"><p>...`

### 4.6 dot (Graphviz, build time)

blocks/diagrams.go:17-66; exec.go:46-61.

- input = `source(body or <Root>/diagrams/<src>.dot)` (4.9).
- `svg = render(env, "dot", input, run)` with run = execute `<dot> -Tsvg`,
  source on stdin, stdout captured; non-zero exit -> error
  `<binary>: <trimmed stderr, or the Go exec error if stderr is empty>`.
- dot binary (resolved once, `sync.Once`): `exec.LookPath("dot")`; else on
  Windows `C:\Program Files\Graphviz\bin\dot.exe` if it exists; else error
  `graphviz not found: install it, or put dot on PATH`.
- Output: `<figure class="diagram diagram--dot">` + cleaned SVG +
  [figcaption] + `</figure>`. No script. Registered but unused by the
  site's content (only `pair` uses Graphviz).

### 4.7 bytefield (npx bytefield-svg, build time)

blocks/diagrams.go:71-110.

- input = `source(body or <Root>/diagrams/<src>.edn)`.
- `svg = render(env, "bytefield", input, run)` with run: write input to a
  temp file `os.CreateTemp("", "press-*.edn")` (the OS temp dir, not
  CacheDir); execute `<npx> --yes bytefield-svg -s <tempfile>` with EMPTY
  stdin; capture stdout; delete the temp file.
- npx binary: on Windows `exec.LookPath("npx.cmd")` if found, else literally
  `npx`.
- Output: `<figure class="diagram diagram--bytefield">` + cleaned SVG +
  [figcaption] + `</figure>`. No script. Site usage: swd-protocol.md:137
  `bytefield src=swd-request caption="..."` with an empty body.

### 4.8 pair (Mermaid + Graphviz side by side)

blocks/diagrams.go:115-147.

- Requires `Attr.Has("src")`, else error `needs src=<name> naming the
  graphviz half` (surfaces as `` ```pair: needs src=... ``).
- Graphviz half rendered exactly like dot from `diagrams/<src>.dot` (same
  `dot` cache namespace). The fence body is the Mermaid half.
- Output:
  `<figure class="diagram diagram--pair"><div class="pair"><div class="pair__side"><p class="pair__label">Mermaid<span>in the browser</span></p><pre class="mermaid">`
  + escape(body) +
  `</pre></div><div class="pair__side diagram--dot"><p class="pair__label">Graphviz<span>at build time</span></p>`
  + cleaned SVG + `</div></div>` + [figcaption] + `</figure>`.
- Script `mermaid`. Five uses in diagrams.md (login, swj, services,
  scraper-er, rebuild), all with `caption=`.

### 4.9 source(): `src=` resolution

exec.go:82-95. If attr `src` is empty/missing -> the fence body. Else read
`<Root>/diagrams/<src><ext>` (ext `.dot` for dot/pair, `.edn` for
bytefield); read error -> `src="<name>": <os error>`. When `src` is set the
body is ignored for Graphviz/Bytefield; for Pair the body is always the
Mermaid half and `src` names the Graphviz half. site/diagrams/ contains
login.dot, rebuild.dot, scraper-er.dot, services.dot, swj.dot, swd-request.edn.

### 4.10 External renderer cache

exec.go:19-44.

- key = hex(sha256(kind + `"\x00"` + src)).
- If `env.CacheDir != ""`: path = `<CacheDir>/<kind>-<hexkey>.svg`; if
  readable, return its bytes as-is (already cleaned).
- Otherwise `run(src)` (error -> build fails), then `cleanSVG(out)`, then
  `MkdirAll(CacheDir, 0755)` and `WriteFile(path, cleaned, 0644)` with errors
  ignored ("a failed cache write is not a failed build").
- No invalidation other than the content hash; nothing ever deletes entries;
  upgrading Graphviz/bytefield-svg does not change output until the source
  changes or the cache dir is deleted. `kind` is `dot` for both Graphviz and
  Pair, `bytefield` for Bytefield.
- Site cache today: 5 `dot-<hash>.svg` + 1 `bytefield-<hash>.svg`.
- The serve watcher excludes Out and CacheDir from its fingerprint (9.2).

### 4.11 cleanSVG

exec.go:63-80.

1. Remove a leading match of `(?s)^\s*(<\?xml.*?\?>\s*)?(<!DOCTYPE[^>]*>\s*)?`.
2. If `<svg` occurs at index > 0, drop everything before it (this removes
   Graphviz's `<!-- Generated by graphviz ... -->` and `<!-- Title: ... -->`
   comments). If `<svg` is absent nothing is cut.
3. In the head up to and including the first `>` (the opening `<svg ...>`
   tag) remove every match of `\s(width|height)="[0-9.]+(pt|px)?"` (numeric
   values with optional pt/px only; `100%` or `12em` survive; only the first
   tag is edited).
4. `bytes.TrimSpace`.

Results: Graphviz starts
`<svg\n viewBox="0.00 0.00 541.00 60.00" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">`;
bytefield starts
`<svg xmlns:svg="http://www.w3.org/2000/svg" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" version="1.0" viewBox="0 0 553 50" >`
(note the leftover space before `>`). The SVG is inlined verbatim inside the
figure.

## 5. Syntax highlighting

### 5.1 Lexer selection and tokenisation

markdown.go:114-128; chroma registry.go:70-98, lexers/lexers.go:42-44,69-85,
coalesce.go:8-35, regexp.go:462-476,518-530.

- `lexers.Get(lang)`: exact name, exact alias, lower-cased name, lower-cased
  alias, then a filename match of `filename.<lang>` and `<lang>` against
  lexer glob patterns (best priority wins); nil -> `lexers.Fallback`, a
  plaintext lexer whose rules (`.+` and `\n`) both yield `Text`. Empty lang
  (indented code) and unknown names give Fallback: no per-token spans at all.
- Resolved names for this site (from a probe run): `""` -> Fallback;
  text/TEXT/txt/plaintext -> plaintext; asm -> GAS; bat -> Batchfile;
  js/javascript -> JavaScript; json -> JSON; c -> C; go -> Go; sh/bash/shell
  -> Bash; console -> Bash Session; md/markdown -> markdown; py/python ->
  Python; toml/yaml/html/css/rust as named. `file=linker.ld` and
  `file=diagrams/login.dot` -> Fallback (no spans in the output).
  `dot`, `mermaid`, `wave`, `note`, `pair`, `bytefield` never reach the
  highlighter (registered renderers).
- The lexer is wrapped in `chroma.Coalesce` (adjacent tokens of the same
  type merged, empty-valued tokens dropped, merging stops once the
  accumulated value reaches 8192 bytes). `Tokenise(nil, text)` uses default
  options: state `root`, `EnsureLF = true` (CRLF and lone CR normalised to
  LF before lexing); lexers with `ensure_nl` (c.xml, javascript.xml) append
  `\n` if missing (fence bodies always end in `\n` anyway).

### 5.2 HTML shape (chroma html formatter, classes mode)

chroma formatters/html/html.go:184-199,231-350,386-422; iterator.go:59-91.

- Output = `<pre class="chroma"><code>` + for each line:
  `<span class="line"><span class="cl">` + tokens + `</span></span>` +
  `</code></pre>`.
- `SplitTokensIntoLines` splits token values after each `\n`; the `\n` stays
  at the end of the line's last token; a trailing line consisting of one
  empty token is dropped. So there is no newline between lines other than
  the `\n` inside the last token, and the block ends
  `...last line\n</span></span></code></pre>`.
- Each token: value escaped with Go `html.EscapeString` (`&` -> `&amp;`,
  `'` -> `&#39;`, `<` -> `&lt;`, `>` -> `&gt;`, `"` -> `&#34;`); wrapped in
  `<span class="X">...</span>` only when its class is non-empty.
- Class lookup (`Formatter.class`): `chroma.StandardTypes[type]`; if absent
  walk to `Parent()` (`t%100 != 0` -> `t/100*100`; else `t%1000 != 0` ->
  `t/1000*1000`; else 0); an entry with an empty string (Text) means NO span.
- StandardTypes (type: class): Text `""`, Whitespace `w`, Error `err`, Other
  `x`; Keyword `k`, KeywordConstant `kc`, KeywordDeclaration `kd`,
  KeywordNamespace `kn`, KeywordPseudo `kp`, KeywordReserved `kr`,
  KeywordType `kt`; Name `n`, NameAttribute `na`, NameBuiltin `nb`,
  NameBuiltinPseudo `bp`, NameClass `nc`, NameConstant `no`, NameDecorator
  `nd`, NameEntity `ni`, NameException `ne`, NameFunction `nf`,
  NameFunctionMagic `fm`, NameProperty `py`, NameLabel `nl`, NameNamespace
  `nn`, NameOther `nx`, NameTag `nt`, NameVariable `nv`, NameVariableClass
  `vc`, NameVariableGlobal `vg`, NameVariableInstance `vi`, NameVariableMagic
  `vm`; Literal `l`, LiteralDate `ld`; String `s`, StringAffix `sa`,
  StringBacktick `sb`, StringChar `sc`, StringDelimiter `dl`, StringDoc `sd`,
  StringDouble `s2`, StringEscape `se`, StringHeredoc `sh`, StringInterpol
  `si`, StringOther `sx`, StringRegex `sr`, StringSingle `s1`, StringSymbol
  `ss`; Number `m`, NumberBin `mb`, NumberFloat `mf`, NumberHex `mh`,
  NumberInteger `mi`, NumberIntegerLong `il`, NumberOct `mo`; Operator `o`,
  OperatorWord `ow`, OperatorReserved `or`; Punctuation `p`; Comment `c`,
  CommentHashbang `ch`, CommentMultiline `cm`, CommentPreproc `cp`,
  CommentPreprocFile `cpf`, CommentSingle `c1`, CommentSpecial `cs`; Generic
  `g`, GenericDeleted `gd`, GenericEmph `ge`, GenericError `gr`,
  GenericHeading `gh`, GenericInserted `gi`, GenericOutput `go`,
  GenericPrompt `gp`, GenericStrong `gs`, GenericSubheading `gu`,
  GenericTraceback `gt`, GenericUnderline `gl`. Wrapper types: PreWrapper
  `chroma`, Line `line`, CodeLine `cl` (LineNumbers `ln`, LineNumbersTable
  `lnt`, LineHighlight `hl`, LineTable `lntable`, LineTableTD `lntd`,
  LineLink `lnlinks`, Background `bg` are never emitted in HTML here).
- Empty body -> `<pre class="chroma"><code></code></pre>`.
- Example (out/golang-notes/index.html):
  `<pre class="chroma"><code><span class="line"><span class="cl"><span class="kn">package</span><span class="w"> </span><span class="nx">main</span>`;
  C example (out/swd-protocol/index.html:160):
  `<span class="k">static</span> <span class="kt">void</span> <span class="nf">swd_clock_out</span><span class="p">(</span><span class="kt">int</span> <span class="n">bit</span><span class="p">)</span>`
  (note the C lexer emits bare spaces as Text with no span, while Go emits
  `<span class="w"> </span>`).
- Exact tokenisation per language depends on chroma v2.27.0's XML lexers
  (C, GAS, Go, JavaScript, JSON, Batchfile, plaintext for this site) plus
  Coalesce; byte-identical output requires reproducing them.

### 5.3 syntax.css generation

markdown.go:130-149; build.go:181-194; chroma formatters/html/html.go:451-615.

- `Renderer.SyntaxCSS()` = `Formatter.WriteCSS(style)` with every line
  dropped whose `TrimSpace`d text starts with `/* Background */ .bg` or
  `/* PreWrapper */ .chroma {`; remaining lines re-joined with `\n` (the
  trailing newline chroma emitted survives as an empty last element, so the
  file ends with `\n`).
- WriteCSS format: each rule `/* <TokenTypeName> */ .chroma .<class> { <css> }\n`;
  first Background (`.bg`) and PreWrapper (`.chroma`) (both dropped), then
  every styled token type sorted by numeric TokenType value, skipping types
  with an empty class; css = `color: #rrggbb`, `background-color: #rrggbb`,
  `font-weight: bold`, `font-style: italic`, `text-decoration: underline`
  joined by `; `, with fixed prefixes for Line (`display: flex;`),
  LineNumbers/LineNumbersTable (`white-space: pre; -webkit-user-select: none;
  user-select: none; margin-right: 0.4em; padding: 0 0.4em 0 0.4em;` plus
  colour), LineTable, LineTableTD, LineLink.
- Written to `out/syntax.css`. The exact 76-line result for `github-dark` is
  out/syntax.css: first line `/* Error */ .chroma .err { color: #f85149 }`,
  last line `/* TextWhitespace */ .chroma .w { color: #6e7681 }`, trailing
  newline. The port can embed that file verbatim rather than re-derive it.

## 6. Templates: loading, composition, functions, data context

### 6.1 Loading (theme.Load)

press/internal/theme/theme.go:48-93; build.go:77-79.

- `filepath.Glob(<templates>/*.html)` (lexically sorted). `base.html` must
  exist (`os.Stat`) else `no base.html in <dir>`.
- Classification by basename: `base.html` skipped; a name starting with `_`
  is a partial; anything else is a page template. Zero page templates ->
  `no page templates in <dir>`.
- For EVERY page template a separate set is parsed: files = `[base.html,
  partials... (glob order), page]`;
  `template.New("base.html").Funcs(funcs).ParseFiles(files...)`; stored in
  `set[basename of page]`. A parse error is `<page basename>: <err>`, wrapped
  by Build as `templates: ...`. A partial's parse error is therefore
  attributed to the first page template in sorted order (`index.html:` for
  this site).
- Templates are read from disk on every Build (not embedded).
- `Has(name)` reports whether a page template exists (used for fallbacks).
- This site's sets: index.html, page.html, post.html, project.html,
  projects.html, tags-list.html, tags-single.html, writing.html; partial
  `_macros.html`.

### 6.2 Composition (Go html/template semantics)

theme.go:102-112; Go text/template template.go:227-238, helper.go:62-94.

- `Render(name, ctx)`: `set[name]` missing -> `no template "<name>" in
  <dir>`; executes the template NAMED `base.html` in that set with `ctx` as
  the sole argument; error -> `<name>: <err>`.
- Each parsed file becomes a template named by its basename; base.html is
  the root. base.html has `{{block "title" .}}{{.Config.Title}}{{end}}`,
  `{{block "description" .}}{{.Config.Description}}{{end}}` and
  `{{block "content" .}}{{end}}`; a block is a define plus an immediate
  `{{template}}` call. Page templates `{{define "title"}}`, `{{define
  "description"}}`, `{{define "content"}}` and, because the page file is
  parsed AFTER base.html, its definitions replace the block defaults (a
  later definition replaces an earlier one unless the new tree is empty and
  an old non-empty one exists). A page that does not define title or
  description (index.html) gets the base defaults (`<title>Josip
  Seketa</title>`).
- Partials contribute only their `{{define}}`d templates (`post_row` with
  dot = `*Page`, `project_card` with dot = `Card`). Top-level text outside
  defines in page/partial files is never executed.
- Nothing uses `{{-`/`-}}` trimming, so every newline and indentation
  around an action is preserved verbatim (section 7.4).
- The rendered bytes are written exactly as produced. base.html ends with
  `</html>\n`, so every page ends with a newline.

### 6.3 Function map (theme.go:114-136,143-215)

| name | signature | semantics |
|---|---|---|
| `url` | `(p string) string` | `@/` prefix: `Site.Resolve(p)` -> URL, or `/` on a miss. Otherwise `"/" + TrimPrefix(p, "/")` (ONE leading slash trimmed, no trailing slash added): `tags` -> `/tags`, `main.css` -> `/main.css`, `//x` -> `//x`, `""` -> `/`. Never prefixed with base_url. |
| `abs` | `(base, p string) string` | `TrimSuffix(base, "/") + url(p)`. Unused by the theme. |
| `sri` | `(rel string) string` | reads `Site.Root/<rel>` (slashes converted with `filepath.FromSlash`; the theme passes `static/js/...`, relative to the SITE root, not the output); any read error -> `""` (no build failure); else `"sha384-" + base64.StdEncoding` (padded, with `+` `/` `=`) of `sha512.Sum384(bytes)`. |
| `date` | `(layout string, t time.Time) string` | `""` if `t.IsZero()`, else `t.Format(layout)` in t's own zone. Layouts used: `2006-01-02`, `Jan 2006`, `02 January 2006`, `2006`. |
| `lower` | `strings.ToLower` | Unicode lowercase (`Active` -> `active`). |
| `prefix` | `strings.HasPrefix(s, p)` | nav highlighting. |
| `pad2` | `(i int) string` | `fmt.Sprintf("%02d", i)`. |
| `plural` | `(n int, word string) string` | `word` if `n == 1` else `word + "s"`. |
| `inc` | `(i int) int` | `i + 1`. |
| `take` | `(n int, pages []*Page) []*Page` | `pages[:min(n, len)]`. |
| `last` | `(pages []*Page) *Page` | last element or nil. |
| `byYear` | `([]*Page) []YearGroup{Year string; Pages []*Page}` | walks in given order; starts a new group whenever `p.Date.Format("2006")` differs from the previous group's Year (consecutive grouping, NOT a sort). |
| `card` | `(p *Page, i, n int) Card` | `Card{Project: p, Index: i, PostCount: n}`. |
| `postsFor` | `Site.PostsFor` | 2.11 |
| `ownerOf` | `Site.OwnerOf` | 2.11 |
| `tagURL` | `(name string) string` | `"/tags/" + Slugify(name) + "/"`. |
| `str` | `(e Extra, k string) string` | `e.Str(k)` |
| `list` | `(e Extra, k string) []string` | `e.List(k)` |
| `flag` | `(e Extra, k string) bool` | `e.Bool(k)`. Unused by the theme. |

Builtins used: `index` (map lookup; a missing key yields a nil `*Section`
and a later field access on it is an execution error), `len` (slices), `or`
(first truthy argument, else the last).

### 6.4 Template-language semantics the theme relies on

Go text/template exec.go:324-350,1115-1134.

- Truthiness (`isTrue`): string/slice/map/array true iff `len > 0`;
  pointer/interface true iff non-nil; ints/floats true iff `!= 0`; bool as
  is; struct always true; invalid/untyped nil false.
- Printing: `fmt.Sprint` semantics after dereferencing pointers; ints in
  decimal; `template.HTML` as its string.
- `{{with x}}` rebinds dot and skips the body when `x` is falsy.
- `{{range}}` over slices with optional `$i, $p :=` (0-based index).
- Variables declared with `{{$x := expr}}` are visible for the rest of the
  enclosing define; a declaration emits nothing.
- Method calls with arguments (`.Page.NeedsScript "mermaid"`) and niladic
  methods used like fields (`.PageCount`).
- Nested parenthesised calls up to four deep.
- `{{/* ... */}}` emits nothing.
- No pipelines (`|`), no `eq/ne/lt/gt/le/ge`, `and/not`, `printf/print`,
  `html/js/urlquery`, `slice`, `call`, range-else, map ranges are used.

### 6.5 html/template contextual auto-escaping (required for byte-identical output)

Go html/template html.go:27-51,144-176; url.go:34-139; escape.go:203-260;
attr.go:140-175. The theme never uses the `html`/`urlquery`/`js` builtins;
these implicit escapers are the only ones.

1. Text between tags (`htmlEscaper`), RCDATA such as `<title>`
   (`rcdataEscaper`) and quoted attribute values (`attrEscaper`) all apply
   the `htmlReplacementTable` to a plain string: U+0000 -> U+FFFD, `"` ->
   `&#34;`, `&` -> `&amp;`, `'` -> `&#39;`, `+` -> `&#43;`, `<` -> `&lt;`,
   `>` -> `&gt;`; every other rune (including invalid UTF-8 bytes) is copied
   through unchanged.
2. Values of type `template.HTML` (`Page.Content`, `Section.Content`) are
   emitted verbatim in text context (the theme uses them only there; in
   attribute/RCDATA context they would be tag-stripped and normalised).
3. URL-typed attributes -- `href`, `src`, and any attribute whose name (after
   stripping a `data-` prefix) contains `src`, `uri` or `url` -- get, when
   the action is at the very start of the value (`href="{{...}}"`):
   `urlFilter` then `urlNormalizer` then `attrEscaper`; when the action
   follows literal text before any `?` or `#`
   (`href="https://github.com/{{...}}"`, `href="mailto:{{...}}"`):
   `urlNormalizer` then `attrEscaper`. `urlFilter`: if the value has a
   scheme (text before the first `:` containing no `/`) that is not
   http/https/mailto (case-insensitive) the whole value becomes
   `#ZgotmplZ`. `urlNormalizer` (url.go:95-139): for each BYTE leave alone
   ASCII letters/digits, `-` `.` `_` `~`, the reserved set
   `! # $ & * + , / : ; = ? @ [ ]`, and `%` followed by two hex digits;
   every other byte is written as lowercase `%xx`. `attrEscaper` then
   applies table (1), which is why a `&` in a URL would become `&amp;`.
4. A value in a non-URL attribute (`datetime`, `data-status`, `content`,
   `lang`, `title`, `class`, `integrity`) gets `attrEscaper` only. This is
   why the base64 SRI digests appear as `integrity="sha384-...&#43;..."`.
5. Literal template text inside a tag is copied verbatim:
   `{{if prefix .CurrentPath "/projects"}}aria-current="page"{{end}}` gives
   `<a href="/projects/" aria-current="page">projects</a>` on a projects page
   and `<a href="/projects/" >projects</a>` (space before `>`) elsewhere.
6. Observed: descriptions render `'` as `&#39;` (`you&#39;ve`) and `"` as
   `&#34;` (`&#34;blue pill&#34;`) in `<meta content>` and card blurbs
   (out/projects/index.html:92,136); `+` -> `&#43;` in the integrity
   attributes (out/diagrams/index.html:484-485, 2 occurrences;
   out/swd-protocol/index.html: 1); goldmark's `&rsquo;` inside Content is
   untouched; `mailto:jseketa@gmail.com` and `https://github.com/jseketa`
   pass through the normalizer unchanged. No actions appear inside
   `<script>`, `<style>` or JS/CSS attributes, so no JS/CSS escaping is
   exercised.

### 6.6 Ctx: the single template argument

theme.go:21-40; build.go:112-117.

```
Ctx { Config *config.Config; Site *content.Site; Page *content.Page; Section *content.Section;
      Term *content.Term; Terms []*content.Term; CurrentPath string; Year string }
Card { Project *content.Page; Index int; PostCount int }     // built only by `card`
YearGroup { Year string; Pages []*content.Page }             // produced only by `byYear`
```

`builder.ctx()` makes a fresh Ctx per render: Config, Site, `Terms =
Site.Terms` (Name-sorted, always set), `Year = time.Now().Format("2006")`
in the build machine's local time; Page/Section/Term nil and CurrentPath
`""` until the page kind sets them.

### 6.7 Template selection and context per page kind

build.go:119-179.

| kind | template | Ctx fields | output |
|---|---|---|---|
| page (`writePages`, Site.Pages in walk order) | `section.PageTemplate` if `pg.Section != nil && PageTemplate != "" && theme.Has(it)`, else `page.html`. The page's own `template` key is ignored. | `Page = pg`, `Section = pg.Section` (nil possible), `CurrentPath = pg.URL` | `writePage(pg.URL)`; errors `<pg.Source>: <err>` |
| section (`writeSections`, map order) | `sec.Template` if non-empty and `theme.Has(it)`, else `index.html` | `Section = sec`, `CurrentPath = sec.URL`, Page nil | `writePage(sec.URL)`; errors `section <name>: <err>` |
| tag index (`writeTaxonomies`) | `tags-list.html` (mandatory; missing -> `no template "tags-list.html" in <dir>`) | `CurrentPath = "/tags/"`; Page/Section/Term nil | `tags/index.html`, always written |
| tag page (each Term, Name order) | `tags-single.html` (mandatory) | `Term = term`, `CurrentPath = term.URL` | `writePage(term.URL)`; errors `tag <Name>: <err>` |

For this site: root section (`template = "index.html"`) -> index.html ->
`index.html`; projects (`projects.html`, `page_template = "project.html"`)
-> `projects/index.html` and `projects/<name>/index.html`; writing
(`writing.html`, `page_template = "post.html"`) -> `writing/index.html` and
`<path>/index.html` at root level; about.md -> page.html -> `about/index.html`
(the root section has no page_template; about.md's own `template =
"page.html"` is inert and works only because page.html is the default).

## 7. Template constructs the site's theme actually uses

Verbatim from site/theme/*.html. This is the complete list; a port of the
template language needs equivalents for exactly these and nothing more.

### 7.1 Composition

- Blocks with defaults (base.html:7,8,67):
  `<title>{{block "title" .}}{{.Config.Title}}{{end}}</title>`
  `<meta name="description" content="{{block "description" .}}{{.Config.Description}}{{end}}">`
  `  {{block "content" .}}{{end}}` (two spaces of indentation inside `<main>`)
- Overrides in page templates:
  page.html/post.html/project.html:1-2:
  `{{define "title"}}{{.Page.Title}} - {{.Config.Title}}{{end}}`
  `{{define "description"}}{{if .Page.Description}}{{.Page.Description}}{{else}}{{.Config.Description}}{{end}}{{end}}`
  projects.html:1-2: `{{define "title"}}Projects - {{.Config.Title}}{{end}}`,
  `{{define "description"}}Things I have built, and the notes that came out of them.{{end}}`
  writing.html:1-2: `Writing - {{.Config.Title}}`, `Build logs and notes.`
  tags-list.html:1-2: `Tags - {{.Config.Title}}`, `Browse writing by tag.`
  tags-single.html:1-2: `{{define "title"}}{{.Term.Name}} - {{.Config.Title}}{{end}}`,
  `{{define "description"}}Posts tagged {{.Term.Name}}.{{end}}`
  index.html:1 defines only `content` (gets both defaults).
- Partials (_macros.html:1-7, 9-29):
  `{{define "post_row"}}` ... `{{end}}` (dot = `*Page`) and
  `{{define "project_card"}}` ... `{{end}}` (dot = `Card`). Both bodies start
  with a newline after `{{define}}` and end with a newline before `{{end}}`.
- Partial invocation:
  `{{template "post_row" .}}` (index.html:60, project.html:34,
  tags-single.html:11, writing.html:16) and
  `{{template "project_card" (card $p (inc $i) (len (postsFor (str $p.Extra "slug"))))}}`
  (index.html:45, projects.html:18).
- Comment (base.html:83-85, three lines):
  `{{/* Which libraries a page loads is derived from the blocks it actually`
  `     contains, not declared in front matter - a page cannot drift out of sync`
  `     with a flag it forgot to set. */}}`

Counts (measured over theme/*.html): `{{define}}` 24, `{{block}}` 3,
`{{template}}` 6, `{{if}}` 21, `{{else}}` 9, `{{else if}}` 1, `{{with}}`
10, `{{range}}` 12, comment 1, `{{end}}` 70 (= 24 + 3 + 21 + 10 + 12).

### 7.2 Actions and expressions

- Field paths on Ctx: `.Config.Title` (11x), `.Config.Description` (4x),
  `.Config.DefaultLanguage`, `.Config.Extra` (as an argument), `.Page.Title`
  (6x), `.Page.Description` (3x plus in `if`), `.Page.Content` (3x),
  `.Page.Date`, `.Page.Tags`, `.Page.Extra` (as argument), `.Page.Earlier`,
  `.Page.Later`, `.Page` (in `if`), `.Section.Content` (2x plus in `if`),
  `.Section.Pages`, `.Site.Sections` (argument to `index`), `.CurrentPath`
  (argument to `prefix`), `.Year` (2x), `.Terms`, `.Term.Name` (3x),
  `.Term.Pages`.
- Fields on a rebound dot: `.` (10x), `.Date`, `.URL` (4x), `.Title` (3x),
  `.Tags`, `.Name`, `.PageCount`, `.Year`, `.Pages`; Card fields
  `.Project.URL`, `.Project.Title`, `.Project.Description`, `.Project.Extra`,
  `.Index`, `.PostCount`.
- Variables: index.html:2-4
  `{{$projects := index .Site.Sections "projects"}}`
  `{{$writing := index .Site.Sections "writing"}}`
  `{{$oldest := last $writing.Pages}}`;
  post.html:4 `{{$owner := ownerOf .Page}}`;
  project.html:4 `{{$linked := postsFor (str .Page.Extra "slug")}}`;
  range variables `{{range $i, $p := take 3 $projects.Pages}}` (index.html:44)
  and `{{range $i, $p := .Section.Pages}}` (projects.html:17). Read as
  `$projects.Pages`, `$projects.URL`, `$writing.Pages`, `$writing.URL`,
  `$oldest.Date`, `$owner.URL`, `$owner.Title`, `$p.Extra`, and passed as
  arguments (`take 3 $projects.Pages`, `len $writing.Pages`, `card $p ...`,
  `inc $i`).
- Function calls with string literals, integer literals, field paths,
  variables, and parenthesised nested calls:
  `{{url "main.css"}}`, `{{date "2006-01-02" .Date}}`,
  `{{str .Config.Extra "github"}}`, `{{prefix .CurrentPath "/projects"}}`,
  `{{plural .PostCount "post"}}`, `{{.Page.NeedsScript "mermaid"}}`,
  `{{index .Site.Sections "projects"}}`,
  `{{sri "static/js/mermaid/mermaid.min.js"}}`, `take 3`, `take 4`,
  `{{plural (len $projects.Pages) "project"}}`,
  `{{lower (str .Project.Extra "status")}}`,
  `postsFor (str .Page.Extra "slug")`,
  `card $p (inc $i) (len (postsFor (str $p.Extra "slug")))` (four deep).
- Method calls: `.Page.NeedsScript "mermaid"`, `.Page.NeedsScript "wavedrom"`
  (one string argument on `*Page`); `.PageCount` / `.Term.PageCount` (niladic
  on `*Term`, used like a field).
- `{{or .Page.Earlier .Page.Later}}` (post.html:28): builtin `or` over two
  nillable pointers, used only as an `if` condition.

### 7.3 Control flow, verbatim

- `{{if}}` on: nil-able pointers (`{{if .Page}}` base.html:86, `{{if $owner}}`
  post.html:9, `{{if $oldest}}` index.html:25); slices (`{{if .Tags}}`
  _macros.html:5, `{{if .Page.Tags}}` post.html:15, `{{if .Terms}}`
  tags-list.html:12, `{{if .Section.Pages}}` projects.html:15/writing.html:12,
  `{{if $projects.Pages}}` index.html:42, `{{if $writing.Pages}}`
  index.html:58, `{{if $linked}}` project.html:33); strings
  (`{{if .Page.Description}}`, `{{if .Section.Content}}` projects.html:11 --
  a `template.HTML`); booleans from `prefix`; `{{if or .Page.Earlier .Page.Later}}`.
- `{{else}}` branches: `<p class="empty">No projects yet.</p>` (index.html:48,
  projects.html:21), `<p class="empty">Nothing here yet.</p>` (index.html:62,
  writing.html:19), `<p class="empty">No posts yet.</p>` (project.html:35),
  `<p class="empty">No tags yet.</p>` (tags-list.html:18), and the
  `{{.Config.Description}}` fallbacks.
- Nav (base.html:44-46):
  `<a href="{{url "@/projects/_index.md"}}" {{if prefix .CurrentPath "/projects"}}aria-current="page"{{end}}>projects</a>`
  `<a href="{{url "@/writing/_index.md"}}" {{if prefix .CurrentPath "/writing"}}aria-current="page"{{else if prefix .CurrentPath "/tags"}}aria-current="page"{{end}}>writing</a>`
  `<a href="{{url "@/about.md"}}" {{if prefix .CurrentPath "/about"}}aria-current="page"{{end}}>about</a>`
- Script block (base.html:86-93):
  ```
  {{if .Page}}{{if .Page.NeedsScript "mermaid"}}
  <script defer src="{{url "js/mermaid/mermaid.min.js"}}" integrity="{{sri "static/js/mermaid/mermaid.min.js"}}"></script>
  <script defer src="{{url "js/mermaid/mermaid-config.js"}}" integrity="{{sri "static/js/mermaid/mermaid-config.js"}}"></script>
  {{end}}{{if .Page.NeedsScript "wavedrom"}}
  <script defer src="{{url "js/wavedrom/wavedrom.min.js"}}" integrity="{{sri "static/js/wavedrom/wavedrom.min.js"}}"></script>
  <script defer src="{{url "js/wavedrom/default.js"}}" integrity="{{sri "static/js/wavedrom/default.js"}}"></script>
  <script defer src="{{url "js/wavedrom/wavedrom-init.js"}}" integrity="{{sri "static/js/wavedrom/wavedrom-init.js"}}"></script>
  {{end}}{{end}}
  ```
- `{{with}}` (rebinds dot, skipped when empty):
  `{{with str .Project.Extra "status"}}<span class="p-card__status">{{.}}</span>{{end}}` (_macros.html:13)
  `{{with .Project.Description}}<p class="p-card__blurb">{{.}}</p>{{end}}` (:18)
  `{{with list .Project.Extra "stack"}}` / `<ul class="stack">{{range .}}<li class="pill">{{.}}</li>{{end}}</ul>` / `{{end}}` (:20-22)
  `{{with str .Project.Extra "year"}}<span>{{.}}</span>{{end}}` (:26)
  `{{with str .Page.Extra "status"}}<span class="article__project">{{.}}</span>{{end}}` (project.html:9)
  `{{with str .Page.Extra "year"}}<span class="article__date">{{.}}</span>{{end}}` (:10)
  `{{with str .Page.Extra "repo"}}<a class="article__date" href="{{.}}" rel="noopener">source</a>{{end}}` (:11)
  `{{with list .Page.Extra "stack"}}` ... `{{end}}` (:16-18)
  `{{with .Page.Earlier}}<a class="prev" href="{{.URL}}"><span>earlier</span>{{.Title}}</a>{{end}}` (post.html:30)
  `{{with .Page.Later}}<a class="next" href="{{.URL}}"><span>later</span>{{.Title}}</a>{{end}}` (:31)
- `{{range}}`:
  `{{range take 4 $writing.Pages}}{{template "post_row" .}}{{end}}` (index.html:60)
  `{{range $i, $p := take 3 $projects.Pages}}` (index.html:44)
  `{{range $i, $p := .Section.Pages}}` (projects.html:17)
  `{{range $linked}}{{template "post_row" .}}{{end}}` (project.html:34)
  `{{range .Term.Pages}}{{template "post_row" .}}{{end}}` (tags-single.html:11)
  `{{range byYear .Section.Pages}}` with body `<h2 class="year-head">{{.Year}}</h2>` and `{{range .Pages}}{{template "post_row" .}}{{end}}` (writing.html:13-18)
  `{{range .Terms}}` with `<li><a class="pill" href="{{.URL}}">{{.Name}} <span class="count">{{.PageCount}}</span></a></li>` (tags-list.html:14-16)
  `{{range .Tags}}<span class="pill">{{.}}</span>{{end}}` (_macros.html:5)
  `{{range .Page.Tags}}<a class="pill" href="{{tagURL .}}">{{.}}</a>{{end}}` (post.html:17)
  `{{range .}}<li class="pill">{{.}}</li>{{end}}` inside `with list` (_macros.html:21, project.html:17)
  No `{{range}}...{{else}}` and no map iteration.

### 7.4 The two partials, verbatim

```
{{define "post_row"}}
<li>
  <time datetime="{{date "2006-01-02" .Date}}">{{date "Jan 2006" .Date}}</time>
  <a class="post-list__title" href="{{.URL}}">{{.Title}}</a>
  {{if .Tags}}<span class="post-list__tags">{{range .Tags}}<span class="pill">{{.}}</span>{{end}}</span>{{end}}
</li>
{{end}}

{{define "project_card"}}
<article class="p-card" data-status="{{lower (str .Project.Extra "status")}}">
  <div class="p-card__top">
    <span class="p-card__num">{{pad2 .Index}}</span>
    {{with str .Project.Extra "status"}}<span class="p-card__status">{{.}}</span>{{end}}
  </div>

  <h3 class="p-card__title"><a href="{{.Project.URL}}">{{.Project.Title}}</a></h3>

  {{with .Project.Description}}<p class="p-card__blurb">{{.}}</p>{{end}}

  {{with list .Project.Extra "stack"}}
  <ul class="stack">{{range .}}<li class="pill">{{.}}</li>{{end}}</ul>
  {{end}}

  <div class="p-card__foot">
    <span>{{.PostCount}} {{plural .PostCount "post"}}</span>
    {{with str .Project.Extra "year"}}<span>{{.}}</span>{{end}}
  </div>
</article>
{{end}}
```

### 7.5 What each template reaches for

- base.html: `.Config.DefaultLanguage`, `.Config.Title` (title default,
  brand, alternate-link title, footer), `.Config.Description`,
  `url "fonts/bricolage-latin-var.woff2"`, `url "fonts/publicsans-latin-var.woff2"`,
  `url "main.css"`, `url "syntax.css"`, `url "assets/favicon.ico"`,
  `url "atom.xml"` (2x), `url "@/_index.md"`, `url "@/projects/_index.md"`,
  `url "@/writing/_index.md"`, `url "@/about.md"`, `prefix .CurrentPath`
  with `/projects`, `/writing`, `/tags`, `/about`, footer
  `(c) {{.Year}} {{.Config.Title}}`,
  `href="https://github.com/{{str .Config.Extra "github"}}"`,
  `href="https://linkedin.com/in/{{str .Config.Extra "linkedin"}}"`,
  `href="mailto:{{str .Config.Extra "email"}}"`, `url "js/theme.js"`, then
  the script block above (5 `url` + 5 `sri` calls).
- index.html: `$projects`, `$writing`, `$oldest`; `str .Config.Extra
  "tagline"` / `"headline"`; `.Section.Content` (hero bio); `len
  $projects.Pages` / `len $writing.Pages` printed and passed to `plural`;
  `date "2006" $oldest.Date`; `$projects.URL` / `$writing.URL`; `take 3`
  project cards; `take 4` post rows.
- writing.html: `url "tags"`; `byYear .Section.Pages` -> year headings +
  post rows.
- projects.html: `.Section.Content` (guarded), `.Section.Pages` -> cards.
- post.html: `$owner := ownerOf .Page`; `.Page.Title/.Description/.Date/
  .Tags/.Content/.Earlier/.Later`; `tagURL .`; `date "2006-01-02"` and
  `date "02 January 2006"`.
- project.html: `$linked := postsFor (str .Page.Extra "slug")`; `str
  .Page.Extra "status"/"year"/"repo"`; `list .Page.Extra "stack"`;
  `url "@/projects/_index.md"`.
- page.html: `.Page.Title`, `.Page.Description`, `.Page.Content`.
- tags-list.html: `url "@/writing/_index.md"`; `.Terms` -> `.URL`, `.Name`,
  `.PageCount`.
- tags-single.html: `.Term.Name` (3x); `url "tags"`; `.Term.Pages` -> rows.
- _macros.html: as in 7.4.

### 7.6 Whitespace and exact output shape (byte-identity consequences)

Go templates emit template text verbatim; nothing is trimmed.

1. Nav links: `<a href="/projects/" aria-current="page">projects</a>` when
   current, `<a href="/projects/" >projects</a>` (space before `>`)
   otherwise (out/index.html:44-46).
2. After `<main>` comes a line of exactly two spaces (indentation before
   `{{block "content" .}}`), then the newline after `{{define "content"}}`,
   then one empty line per variable-declaration line. out/index.html:66-72
   is `<main>`, `  `, ``, ``, ``, ``, `<div class="wrap">`: the newline after
   `{{define "content"}}` terminates the two-space line, then the three
   declaration lines (index.html:2-4) and index.html's own blank line 5 give
   the four empty lines. Post pages have `<main>`, `  `, `` (the `$owner`
   line), `<div class="wrap wrap--narrow">`.
3. Each `{{template "post_row" .}}` emits a leading newline, so a
   `          {{range .Pages}}` line yields a 10-space line before the first
   `<li>` and a blank line between rows (out/writing/index.html:77-90).
4. False `{{if}}`/`{{with}}` lines leave their indentation as a
   whitespace-only line: 8 spaces where the `repo` link would go
   (out/projects/stm32-bare-metal/index.html:75), 8 spaces around the tags
   block (out/blinking-a-led/index.html:77).
5. The pager with only one neighbour keeps the other line as 4 spaces
   (out/blinking-a-led/index.html:402-405).
6. Tag cloud items are separated by 6-space lines (out/tags/index.html:78-84).
7. `<script src="/js/theme.js" defer></script>` is followed by two blank
   lines then `</body>` on pages without diagrams (the comment line and the
   `{{if .Page}}` line each leave a newline); with mermaid the two script
   lines sit between those blank lines (out/golang-notes/index.html tail,
   out/diagrams/index.html tail).
8. Every output page ends `</html>\n`.

### 7.7 CurrentPath and nav highlighting

CurrentPath is the output URL. Posts live at root-level URLs because of the
`path` override, so `prefix .CurrentPath "/writing"` is false on every post
page: no nav link gets `aria-current` on `/swd-protocol/` etc.
(out/swd-protocol/index.html:44-46). Tag pages highlight "writing" via the
`else if`. `url "tags"` yields `/tags` (no trailing slash) while the tag
index is written to `tags/index.html`; writing.html:9 and tags-single.html:9
both link to `/tags`.

## 8. Output layout, static copy, CSS, feed

### 8.1 Build pipeline order and failure points

build.go:54-92.

1. `config.Load(Root/config.toml)` [error prefix `config: `]
2. `content.Load(Root)` [`content: `]
3. `markdown.New(...)` with the registry
4. `render()`: every `Page.Body` (walk order) then every `Section.Body`
   (map order) [`<Source>: ` / `section <name>: `]
5. `theme.Load(Templates, site)` [`templates: `]
6. `clean(Out)`
7. in order: `writePages`, `writeSections`, `writeTaxonomies`, `writeCSS`,
   `copyStatic`, `writeFeed`; the first error aborts (output may be partially
   written).
8. `Stats{Pages: len(Pages)+len(Sections)+len(Terms)+1, Duration}`.

Because `clean` runs after content, markdown and templates all succeeded, a
content or template error leaves the previous output untouched.

### 8.2 Output path rule and file modes

build.go:273-284. `writePage(url, data)` = `write(filepath.Join(Trim(url,
"/"), "index.html"), data)`: `/` -> `index.html`, `/tags/` ->
`tags/index.html`, `/projects/learning-go/` ->
`projects/learning-go/index.html`. `write(rel, data)`: `dst = Out/rel`;
`MkdirAll(dir(dst), 0o755)`; `WriteFile(dst, data, 0o644)` (truncate).
Rendered bytes are written exactly as produced.

### 8.3 clean

build.go:251-271. `os.ReadDir(Out)`: does not exist -> no-op (the first
write's MkdirAll creates it); any other error (Out is a file, permission) ->
build fails; else `os.RemoveAll(Out/<entry>)` for every entry, failing on the
first error. The directory itself is never removed (Windows handle safety).
Everything in Out not produced by the build is deleted on every build.

### 8.4 CSS

build.go:181-194; theme/css.go:15-17.

- `main.css` = `lace.Compile(<site>/sass/main.scss)` written to `out/main.css`
  (1391 lines for this site). lace's semantics are out of scope here (see
  the lace area); the port must call an equivalent compiler on the same input
  path.
- `syntax.css` = `Renderer.SyntaxCSS()` (5.3) written to `out/syntax.css`.
- `writeCSS` runs after all HTML has been written, so a lace or chroma error
  leaves pages but no main.css.

### 8.5 Static copy

build.go:196-220. `filepath.Walk(<site>/static, fn)`; `fn`: if `err != nil`
or the entry is a directory -> `return err` (a missing or unreadable static
dir fails the build; directories are not created eagerly). For every
non-directory entry (a symlink is treated as a file and its target content
copied): `rel` relative to static; `dst = Out/rel`; `MkdirAll(dir(dst),
0o755)`; `os.Create(dst)` (0666 before umask, truncating); `io.Copy`. No
filtering of any kind: dotfiles, CNAME, LICENSE, *.css, fonts, js, svg all
copy 1:1 preserving layout. Walk order lexical. Later static files overwrite
page output at the same path. Nothing outside static/ is copied (theme/,
diagrams/, files next to content are not).

For the site this yields `CNAME` (content `seketa.it`), `assets/**` (author
images, covers, favicon.ico, images/*.gif|png), `diagrams/*.svg` (6 legacy
files), `fonts/*.woff2` (8), `giallo.css`, `giallo-dark.css`,
`giallo-light.css`, `js/theme.js`, `js/mermaid/{LICENSE,mermaid-config.js,
mermaid.min.js}`, `js/wavedrom/{LICENSE,default.js,wavedrom-init.js,
wavedrom.min.js}`.

### 8.6 Atom feed: exact bytes

build.go:222-249. Skipped entirely (no file) unless `generate_feeds == true`
AND a section named `writing` exists. `base = TrimSuffix(BaseURL, "/")`
(exactly one slash). LF line endings, two-space indent:

```
<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>{esc(cfg.Title)}</title>
  <id>{base}/atom.xml</id>
  <updated>{time.Now().UTC().Format(RFC3339)}</updated>
  <link rel="self" href="{base}/atom.xml"/>
  <link href="{base}/"/>
  <entry>
    <title>{esc(page.Title)}</title>
    <id>{base}{page.URL}</id>
    <link href="{base}{page.URL}"/>
    <updated>{page.Date.UTC().Format(RFC3339)}</updated>
    <content type="html">{esc(string(page.Content))}</content>
  </entry>
</feed>
```

- One `<entry>` per page of the writing section in section order (date
  desc: diagrams, swd-protocol, golang-notes, microservices, initial-design,
  blinking-a-led). Written even when the section has zero pages.
- No author, summary, categories/tags, published or rights elements.
- `time.RFC3339` = `2006-01-02T15:04:05Z07:00`: UTC prints a trailing `Z`,
  always whole seconds; a zero date prints `0001-01-01T00:00:00Z`.
- `esc` = `text/template.HTMLEscapeString` (funcs.go:599-645): NUL ->
  U+FFFD, `"` -> `&#34;`, `'` -> `&#39;`, `&` -> `&amp;`, `<` -> `&lt;`,
  `>` -> `&gt;`. Nothing else: `+` is NOT escaped (30 raw `+` in atom.xml,
  zero `&#43;`), non-ASCII passes through, existing entities are
  double-escaped (`&mdash;` -> `&amp;mdash;`, `&#34;` -> `&amp;#34;`).
  `base`, page URLs and dates are inserted raw.
- Reference: out/atom.xml:1-12 (`<updated>2026-08-30T00:52:11Z</updated>`,
  `<id>https://seketa.it/diagrams/</id>`, entry
  `<updated>2026-08-21T22:00:00Z</updated>`); file ends `</feed>\n`.

### 8.7 Output tree for this site (71 files)

`index.html`, `about/index.html`, `writing/index.html`,
`projects/index.html`, `projects/{learning-go,ogame-scraper,stm32-bare-metal}/index.html`,
`{blinking-a-led,diagrams,golang-notes,initial-design,microservices,swd-protocol}/index.html`,
`tags/index.html`, `tags/{c,embedded,golang,meta,microservices,nodejs,scraping,stm32,swd,tooling}/index.html`,
`main.css`, `syntax.css`, `atom.xml`, plus the 44 static files of 8.5.

## 9. Serve mode

press/internal/site/serve.go.

### 9.1 Startup and routing (serve.go:24-54; main.go:43-45)

- `r := reloader{opts, log, subs: map[chan struct{}]struct{}{}}`; initial
  `r.build()` (a failure is returned and made fatal by main); print
  `serving http://%s/ (rebuilds on change, reloads open pages)\n` with addr;
  `go r.watch()`; mux: `/_reload` (exact path) -> the SSE handler; `/` ->
  `r.pages(http.FileServer(http.Dir(Out)))`; `http.ListenAndServe(addr,
  mux)`. addr = `127.0.0.1:<port>`.
- `reloader.build()` runs a full `Build` and prints `%d pages -> %s in %v\n`
  with `Duration.Round(time.Millisecond)`.

### 9.2 Change detection (serve.go:56-106)

- `watch()`: `last = fingerprint()`; forever: sleep 300 ms; `now =
  fingerprint()`; if equal continue; `last = now` (set BEFORE building, so a
  failing build is not retried until another change); `build()`; on error
  print `build failed: %v\n` and continue (open pages keep what they have);
  on success `notify()`.
- `fingerprint()`: `skip = {abs(Out), abs(CacheDir)}`; FNV-1a 64-bit;
  `filepath.WalkDir(Root, fn)`; walk errors ignored; for a directory:
  `SkipDir` if `abs(path)` is in skip, or its name is `.git`, `node_modules`
  or `public` (Zola's output), or (`path != Root` and the name starts with
  `.`); for a file: `d.Info()` (error -> ignored) then write
  `"%s\x00%d\x00%d\n"` of (path, `ModTime().UnixNano()`, `Size()`) into the
  hash; return `Sum64()`. Only equality matters. Hidden FILES are included;
  a `-templates` directory outside Root is NOT watched; the default
  `.press-cache` is skipped both by the abs match and by the dot rule.

### 9.3 /_reload SSE endpoint (serve.go:108-149)

- If the ResponseWriter is not an `http.Flusher`: 500 `streaming unsupported`.
- `ch := make(chan struct{}, 1)`; register in `subs` under the mutex;
  deregister on return.
- Headers `Content-Type: text/event-stream`, `Cache-Control: no-cache`.
  Write `": ready\n\n"` and flush. Loop: on a tick write `"data:
  reload\n\n"` and flush; on `req.Context().Done()` return.
- `notify()`: under the mutex, non-blocking send to every subscriber
  (`select` with `default`): a client that has not consumed the previous
  event gets no second one (events coalesce).

### 9.4 Injected reload script and HTML handler (serve.go:151-183)

- `reloadScript` is exactly:
  `<script>(()=>{let dropped=false;const es=new EventSource("/_reload");es.onmessage=()=>location.reload();es.onerror=()=>{dropped=true};es.onopen=()=>{if(dropped)location.reload()}})()</script>`
  (reload on any message, and when the connection re-opens after having
  dropped, i.e. press restarted).
- `pages(files)`: `p = path.Clean("/" + req.URL.Path)`; if `req.URL.Path`
  ends with `/`: `p = path.Join(p, "index.html")`; if `p` does not end with
  `.html`: delegate to the file server (assets, and `/foo` without a slash,
  which FileServer 301-redirects to `/foo/` when foo is a directory).
  Otherwise read `Out/<p>` (slashes to OS separators); on read error
  delegate to the file server (its 404 / redirects apply). Inject: find the
  LAST `</body>` and insert the script immediately before it; if there is no
  `</body>` append the script at the end. Headers `Content-Type: text/html;
  charset=utf-8`, `Cache-Control: no-store`; status 200. Files on disk are
  never modified. A direct request for `/x/index.html` is served with
  injection too.

## 10. Edge cases

Config and CLI
- Config/front-matter TOML keys match case-insensitively as a fallback;
  unknown keys never error; wrong-typed values do.
- `base_url`: exactly one trailing `/` is trimmed (`TrimSuffix`, not
  `TrimRight`); `https://seketa.it//` would produce `https://seketa.it//atom.xml`.
- A misspelled `markdown.highlighting.theme` silently produces the `swapoff`
  style and a different syntax.css (`styles.Get` never returns nil).
- `taxonomies`, `markdown.highlighting.style`, `feed_filenames`,
  `build_search_index`, `render_emoji` and the section-level `generate_feeds`
  are all inert.
- The stats count (24) is pages + sections + terms + 1, not files written.

Content and front matter
- `date = "2018-06-02"` as a quoted string fails the build; `weight = 1.0`
  or `weight = "1"` also fail (integer only). Dates with missing leading
  zeros are rejected.
- Feed entry dates are local midnight shifted to UTC using the offset in
  force on the BUILD DAY (`localOffset` computed once from `time.Now()`):
  in CEST every post shows `T22:00:00Z` of the previous day; in CET
  `T23:00:00Z`. HTML pages are unaffected. Not reproducible across
  machines/time zones.
- A page with no date in a date-sorted section sorts last (zero time),
  appears in atom.xml with `<updated>0001-01-01T00:00:00Z</updated>`, and the
  `date` function prints `""` for it (`<time datetime="">`).
- Any file whose relative path merely ENDS with `_index.md` (e.g.
  `writing/my_index.md`) is treated as the section for its directory and
  silently replaces the real one.
- Two files producing the same URL (two posts with the same `path`, or a
  static file at `static/about/index.html`) do not error: the later write
  wins (pages in walk order, then sections, then tags, then static copy).
- `path` override applies to sections too and is not normalised beyond
  trimming slashes.
- A page-level `template` key is decoded but never used; page templates come
  only from the section's `page_template`, falling back to page.html when
  the section is nil or the named template is absent (no error). A section
  `template` naming a missing file silently falls back to index.html.
- Pages in a directory without `_index.md` get `Section = nil`: rendered with
  page.html, listed nowhere, no section index written. Nested dirs need
  their own `_index.md` (section name `a/b`, URL `/a/b/`); a page in `a/b`
  never rolls up into section `a`.
- Only `taxonomies.tags` is read. `/tags/index.html` is written
  unconditionally, even with zero tags (renders `No tags yet.`).
- Tags are matched case-sensitively and untrimmed (`Go` and `go` are two
  terms) but both slugify to `/tags/go/` and the second overwrites the
  first's page. A tag repeated within one page's list counts twice. A tag of
  only non-ASCII/punctuation characters slugifies to `""` giving URL
  `/tags//`, written to `tags/index.html`, clobbering the tag index.
- Tagged pages of ANY section (projects, root pages) would appear on term
  pages and in counts; today only writing posts carry tags.
- Non-.md files under content/ are ignored entirely, not copied; `.MD` is
  ignored; dot-directories under content/ are walked.
- `Extra.List` keeps only string elements; `Extra.Str` on a non-string
  returns `""`.
- `Resolve("@/x")` is a linear scan returning the first Source match; `url`
  of an unresolvable `@/` ref emits `/` rather than failing.
- Content files are LF today, but `core.autocrlf = true` is set; CRLF input
  is tolerated by the splitter (leading `\r\n` trimmed from the body, stray
  `\r` inside the TOML accepted) and chroma normalises CRLF before lexing;
  goldmark's own CR handling is untested here.
- `generate_feeds = true` in writing/_index.md is ignored; the feed is gated
  only by config `generate_feeds` and the literal section name `writing`.

Markdown and renderers
- `Result.Scripts` order is Go map order (nondeterministic); only membership
  is used.
- A page whose only mermaid/wave block sits inside a ```note body still gets
  the script (nested Convert writes into the same set).
- `fragment()` is only safe inside `Render`: outside it `r.scripts` is nil
  and a script-bearing block would panic (never happens in practice).
- Heading ids use the raw last source line: inline markup characters vanish
  but their text stays; link URLs are included; non-ASCII dropped;
  separators not collapsed; uniqueness restarts per Convert (page vs. note
  fragments can collide).
- Three different escapers must all be matched: goldmark text (`&quot;`,
  `'` untouched), chroma tokens (`&#34;`, `&#39;`), blocks.escape (`&quot;`,
  `'` untouched); and html/template adds `&#43;` for `+`.
- Unknown or empty highlight language -> Fallback plaintext: still wrapped
  in `<pre class="chroma"><code><span class="line"><span class="cl">...`
  but with no token spans. Empty fence body -> `<pre class="chroma"><code></code></pre>`.
- The leading-space info strings (` file=linker.ld`, ` file=diagrams/login.dot`)
  lose their file bar because `file=...` becomes the language name.
- ParseInfo: an unterminated quote swallows the rest of the line; `key=`
  yields an empty value which `Attr.Get` treats as absent; a leading or lone
  `=` ends attribute parsing silently; bare-word flags map to `key=key` but
  no renderer reads any other key (e.g. `class=wide` is parsed and ignored).
- Typographer output is entity text emitted raw (IsCode); do not re-escape.
- With `WithUnsafe`, `javascript:`/`data:`/`vbscript:`/`file:` URLs and raw
  HTML pass through, only NUL is replaced.
- Indented code blocks are always plain (Fallback lexer) and trailing blank
  lines are trimmed by the parser.
- Footnote backlinks are inserted inside the last paragraph of the note
  preceded by `&#160;`; multiple references to one note produce `fnref1:N`,
  `fnref2:N` ids and multiple backlinks.
- cleanSVG only strips width/height with purely numeric values (optional
  pt/px), only in the first tag; if `<svg` is absent nothing is cut.
- `render()` returns cached bytes without re-cleaning; entries never expire.
- Bytefield's temp file goes in the OS temp dir; stdin to npx is empty; the
  CLI is `npx --yes bytefield-svg -s <file>` (`npx.cmd` via PATH on Windows).
- Graphviz binary lookup is cached process-wide; the Windows fallback path is
  hard-coded to `C:\Program Files\Graphviz\bin\dot.exe`.
- Pair without `src=` is a build error; Graphviz/Bytefield with `src=` ignore
  the fence body (which may be empty).
- Captions are HTML-escaped only for `& < > "`; apostrophes stay raw and no
  typographer runs on them.

Templates and output
- html/template escapes `+` as `&#43;` in text, RCDATA and attribute
  contexts. Visible today only in `integrity="sha384-..."`, but any
  title/description/tag containing `+` (e.g. `C++`) would render `C&#43;&#43;`.
- `template.HTML` bypasses escaping only in text context; every other string
  (including ones already containing entities) is escaped again.
- `url()`: `TrimPrefix` removes exactly one leading slash (`//x` stays
  protocol-relative); `""` -> `/`.
- `sri()` returns `""` (`integrity=""`) rather than failing when the file is
  missing.
- `Ctx.Year` is the build-machine local year (footer `(c) 2026`); output is
  not reproducible across calendar years.
- `byYear` groups consecutive runs only; a weight-sorted section passed to
  it could repeat year headings. `take(n)` clamps; `last()` on an empty
  slice returns nil (falsy).
- `index .Site.Sections "projects"` / `"writing"` with a missing section
  returns a nil `*Section`; index.html then fails at `$projects.Pages` (nil
  pointer evaluation), so index.html hard-requires both sections. `last
  $writing.Pages` guards only the empty-slice case.
- tags-list.html and tags-single.html are required templates even for a site
  with no tags.
- `{{if .Tags}}` false in `post_row` would leave a line of exactly two
  spaces; none of the `empty` branches, the `repo` link, or a
  description-less page appear in the current output.
- Section write order is Go map order (random per run); output content is
  unaffected. `Site.Pages` order is walk order.
- Partials are parsed into every page template's set independently; a
  partial's parse error is attributed to `index.html: ...`.
- `clean()` keeps the output directory; if Out is a regular file the build
  fails at ReadDir. `copyStatic`: a missing `static/` fails the build;
  symlinks are copied as files; CNAME is just a static file.
- Feed `<updated>` at the top level is `time.Now()`, so atom.xml differs on
  every build (70 of 71 files identical between two builds).
- Feed escaping: `'` -> `&#39;`, `"` -> `&#34;`, `+` untouched, entities
  double-escaped; a base_url containing `&` would produce invalid XML.

Serve
- Initial build failure is fatal; later failures only log `build failed: ...`
  and the last-good output stays served. `last` is updated before the build,
  so an unchanged-but-broken tree is not rebuilt until the next edit.
- Directories named `public` anywhere are skipped; hidden directories are
  skipped except the root; hidden files are included; a `-templates` dir
  outside `-site` is not watched.
- Injection targets the LAST `</body>`; HTML without `</body>` gets the
  script appended; `/x/index.html` requested directly is served with
  injection; `/_reload` matches the exact path only.
- One-slot buffered channel per client: rapid rebuilds coalesce into one
  `reload` event; `: ready` is written first to flush headers.

## 11. Open questions

Things the readers could not determine or that need a decision by the port's
author. The readers' four sets are merged and deduplicated.

Reproducibility
1. Feed dates: reproduce the timezone-dependent value (local midnight at the
   build-day offset converted to UTC, `2026-08-21T22:00:00Z` for
   `2026-08-22`) or emit a stable value (`2026-08-22T00:00:00Z`)?
   Byte-for-byte parity requires reading the host's current UTC offset at
   startup exactly as BurntSushi does. The same decision covers whether TOML
   local dates are treated as local midnight (matches current HTML and the
   off-by-one feed) or as UTC (changes atom.xml only).
2. The feed's top-level `<updated>` is the build time; byte-identical output
   is impossible across builds. Keep `now`, or switch to the newest post date
   (which changes output vs. public-press)?
3. `{{.Year}}` is the build-time year. Keep, or inject a fixed value for
   reproducibility tests?
4. Must the Rust output be byte-identical (whitespace-only lines from Go's
   no-trim template semantics, the `<a href=... >` space, `&#43;` escaping,
   the URL normalizer and `#ZgotmplZ` filter) or only DOM-equivalent? This
   decides whether the new template language needs Go-style verbatim text
   handling. Today the only observable `+` difference would be the integrity
   attributes on diagram pages. (Note: press-rs/docs/templates.md already
   specifies standalone-line removal, which is a deliberate departure.)

Highlighting and Markdown
5. Byte-identical highlighting requires reproducing chroma v2.27.0's XML
   lexer rules for C, GAS, Go, JavaScript, JSON, Batchfile and plaintext plus
   Coalesce merging. Port the lexers, shell out, or accept token-span
   differences? The CSS classes are the contract; span boundaries affect
   diffs against public-press.
6. goldmark's CommonMark core (list tightness, HTML block detection,
   reference definitions, entity resolution) is assumed spec-conformant; a
   Rust parser (comrak, pulldown-cmark) will differ in newline placement
   (goldmark emits `\n` after every block close tag, `<li>\n` only when the
   first child is not a TextBlock). The conventions are extracted above but
   should be diffed against public-press.
7. The github-dark style entries were not transcribed token by token; the
   recommendation is to embed out/syntax.css verbatim (76 lines, trailing
   newline), which is exactly what press writes.
8. Should the ` file=linker.ld` / ` file=diagrams/login.dot` quirk (no file
   bar because the leading-space info string makes `file=...` the language)
   be preserved for parity, or fixed in content/port? The author almost
   certainly intended a file bar.

Model quirks
9. Should the `_index.md` SUFFIX rule, case-insensitive TOML key matching,
   and the `path` override on sections be preserved or tightened? None are
   exercised by the current site.
10. Term pages include tagged pages from any section; confirm whether the
    port should scope terms to the writing section.
11. Windows file order: `filepath.Walk` sorts names byte-wise; confirm the
    Rust walker sorts explicitly (`read_dir` order is unspecified) since
    stable-sort tie-breaking and Resolve depend on it.
12. Keep Go text/template's index-on-map returning nil for a missing key
    (with a later crash), or fail immediately / make index.html tolerant of a
    missing section?
13. `abs(base, p)` and `flag(e, k)` are in the func map but unused: port or
    drop?
14. Section write order is nondeterministic in Go; the port may pick any
    order -- confirm nothing downstream depends on it.
15. Config keys that are inert in Go (`taxonomies`, `markdown.highlighting
    .style`, `feed_filenames`, `build_search_index`, `render_emoji`,
    section-level `generate_feeds`) can be dropped, but the port must still
    tolerate unknown keys without error.

Serve and output
16. Must the stats line match Go's `Duration` formatting exactly, or only the
    `N pages -> dir in T` shape?
17. lace's compile semantics for sass/main.scss (30,849 bytes -> 1391-line
    main.css) are covered by the lace area; the port must call an equivalent
    compiler at the same input path.
