//! The rules that are easy to get subtly wrong: the computed-operand rule,
//! spacing in symbolic output, literal spelling, spans and scopes.

fn compile(src: &str) -> Result<String, lace::Error> {
    lace::Compiler::new()
        .load(Box::new(|_| Err("no such file".into())))
        .log(Box::new(std::io::sink()))
        .compile(src, "t.scss")
}

fn check(cases: &[(&str, &str)]) {
    for (input, want) in cases {
        match compile(input) {
            Ok(got) => assert_eq!(&got, want, "input: {input}"),
            Err(e) => panic!("{input}\n  error: {e}"),
        }
    }
}

#[test]
fn values() {
    check(&[
        ("a { font: 12px/1.5 serif; }", "a {\n  font: 12px/1.5 serif;\n}\n"),
        (
            "a { grid-area: 1 / 3; margin: 0 -1px; x: .55s; y: 1E3; z: +.5; }",
            "a {\n  grid-area: 1 / 3;\n  margin: 0 -1px;\n  x: .55s;\n  y: 1E3;\n  z: +.5;\n}\n",
        ),
        (
            "a { width: 100% / 3; content: \"a\" + b; x: not y; u: U+4??; }",
            "a {\n  width: 100% / 3;\n  content: \"a\" + b;\n  x: not y;\n  u: U+4??;\n}\n",
        ),
        (
            "a { b: var(--x,); c: rgb(0 0 0 / 50%); d: url(data:image/svg+xml;charset=utf8,%3Csvg%3E); }",
            "a {\n  b: var(--x,);\n  c: rgb(0 0 0 / 50%);\n  d: url(data:image/svg+xml;charset=utf8,%3Csvg%3E);\n}\n",
        ),
        (
            "@property --angle { syntax: \"<angle>\"; inherits: false; initial-value: 0deg; }",
            "@property --angle {\n  syntax: \"<angle>\";\n  inherits: false;\n  initial-value: 0deg;\n}\n",
        ),
        ("@layer base {} @layer a, b; a { b: c }", "@layer base {}\n\n@layer a, b;\n\na {\n  b: c;\n}\n"),
        (
            "a { --x: { a: b }; --y: \"a;}b\"; --z: ; --w:   1px   2px  ; }",
            "a {\n  --x: { a: b };\n  --y: \"a;}b\";\n  --z: ;\n  --w: 1px 2px;\n}\n",
        ),
        (
            "a { b: [full-start] 1fr [full-end]; c: if(style(--wide: true): 100%; else: 50%); }",
            "a {\n  b: [full-start] 1fr [full-end];\n  c: if(style(--wide: true): 100%; else: 50%);\n}\n",
        ),
        (
            "$g: 8px; a { p: $g * 2 ($g / 2); q: $g/2; r: ($g / 2); s: 1px + 2px + $g; }",
            "a {\n  p: 16px 4px;\n  q: 8px/2;\n  r: 4px;\n  s: 11px;\n}\n",
        ),
        (
            "$x: 2px; a { w: calc(100% - $x); v: calc(var(--w) + $x); u: calc((100% - $x) / 2); t: calc(100%-$x); }",
            "a {\n  w: calc(100% - 2px);\n  v: calc(var(--w) + 2px);\n  u: calc((100% - 2px) / 2);\n  t: calc(100%-2px);\n}\n",
        ),
        (
            "$a: 1fr; $d: .55s; a { x: #{$a}/2; y: $d; z: $d * 2; m: -$a; n: #{$a} + 1; }",
            "a {\n  x: 1fr/2;\n  y: .55s;\n  z: 1.1s;\n  m: -1fr;\n  n: 1fr + 1;\n}\n",
        ),
        (
            "$n: null; a { b: $n; c: 1px $n 2px; d: $n $n; e: min(1px, $n); }",
            "a {\n  c: 1px 2px;\n  e: min(1px);\n}\n",
        ),
        (
            "$g: 1px; a { b: 1 -2; c: 1 - 2; d: 1-2; e: (1 - 2); f: (1 -2); g: $g + 2em; h: (\"a\" + b); }",
            "a {\n  b: 1 -2;\n  c: 1 - 2;\n  d: 1-2;\n  e: -1;\n  f: 1 -2;\n  g: 1px + 2em;\n  h: (\"ab\");\n}\n",
        ),
        ("$w: 640px; a { @media (min-width: #{$w}) { b: c } }", "a {\n  @media (min-width: 640px) {\n    b: c;\n  }\n}\n"),
        ("/* top */\na { /* in */ b: c; // line\n d: e /* v */ f; }", "/* top */\n\na {\n  /* in */\n  b: c;\n  d: e f;\n}\n"),
        ("a { b: \"http://x\"; c: url(//cdn/x.png); } // tail", "a {\n  b: \"http://x\";\n  c: url(//cdn/x.png);\n}\n"),
        ("a { @if false { b: c } } d { e: f }", "d {\n  e: f;\n}\n"),
        ("a {\n  --home: https://x.y;\n  --next: 1;\n}", "a {\n  --home: https: --next: 1;\n}\n"),
        ("$e: (); a { x: $e; y: z; }", "a {\n  y: z;\n}\n"),
    ]);
}

#[test]
fn scopes_and_control() {
    check(&[
        ("$i: 0; @while $i < 3 { .w-#{$i} { w: $i } $i: $i + 1; }", ".w-0 {\n  w: 0;\n}\n\n.w-1 {\n  w: 1;\n}\n\n.w-2 {\n  w: 2;\n}\n"),
        ("@for $i from 3 through 1 { a { b: $i } } @for $i from 1 to 3 { .x-#{$i} { y: $i } }", ".x-1 {\n  y: 1;\n}\n\n.x-2 {\n  y: 2;\n}\n"),
        ("@each $k, $v in (a: 1, b: 2,) { .#{$k} { v: $v } }", ".a {\n  v: 1;\n}\n\n.b {\n  v: 2;\n}\n"),
        (
            "@each $p in (a: 1) { x { y: $p } } @each $a, $b in (1 2, 3) { x { a: $a; b: $b } }",
            "x {\n  y: a 1;\n}\n\nx {\n  a: 1;\n  b: 2;\n}\n\nx {\n  a: 3;\n}\n",
        ),
        ("$x: 1; @mixin m { $x: 2; } a { @include m; b: $x }", "a {\n  b: 2;\n}\n"),
        ("$x: 1; a { $x: 2; b: $x } c { d: $x }", "a {\n  b: 2;\n}\n\nc {\n  d: 2;\n}\n"),
        ("$x: 1; a { $y: 2; b: $x } c { @if true { $z: 3 } d: $x }", "a {\n  b: 1;\n}\n\nc {\n  d: 1;\n}\n"),
        ("$a: 1; $a: 2 !default; $b: null; $b: 3 !default; a { a: $a; b: $b }", "a {\n  a: 1;\n  b: 3;\n}\n"),
        (
            "@mixin card($p: 1px) { .card { padding: $p; @content; } } @include card { color: red; } @include card(2px);",
            ".card {\n  padding: 1px;\n  color: red;\n}\n\n.card {\n  padding: 2px;\n}\n",
        ),
        ("@function f($n) { @if $n <= 1 { @return 1; } @return $n * f($n - 1); } a { b: f(5) }", "a {\n  b: 120;\n}\n"),
        ("@function g($xs...) { @return length($xs); } a { b: g(1, 2, 3) g() }", "a {\n  b: 3 0;\n}\n"),
        (
            "a { b: floor(2.7px) ceil(-1.5) percentage(.5) unitless(1) quote(x) unquote(\"y\") index(a b c, b) nth(a b, -1); }",
            "a {\n  b: 2px -1 50% true \"x\" y 2 b;\n}\n",
        ),
        (
            "$m: map-merge((a: 1), (b: 2)); a { k: map-keys($m); v: map-values($m); r: map-keys(map-remove($m, a)); h: map-has-key($m, b) map-has-key($m, z); }",
            "a {\n  k: a, b;\n  v: 1, 2;\n  r: b;\n  h: true false;\n}\n",
        ),
        ("$x: 0; @while $x != 1 { $x: $x + 0.1; } a { b: $x }", "a {\n  b: 1;\n}\n"),
    ]);
}

#[test]
fn errors() {
    let cases: &[(&str, &str)] = &[
        ("a {\n  color: red\n  .b { x: y }\n}", "t.scss:2:3: unexpected '{' (missing ';' after a declaration?)"),
        (".card { &__title { a: b } }", "t.scss:1:9: &__title is not CSS"),
        ("$bp: 1px; @media (min-width: $bp) { a { b: c } }", "t.scss:1:30: use #{$bp} to interpolate"),
        ("a { b: $nope }", "t.scss:1:8: undefined variable $nope"),
        ("$x: 1px + 2em;", "t.scss:1:5: 1px + 2em: incompatible units"),
        ("$x: 1 + true;", "t.scss:1:5: 1 + true: undefined operation"),
        ("@mixin m { a: b } .x { @include m { c: d } }", "t.scss:1:24: mixin m does not accept a block"),
        ("@include nope;", "t.scss:1:1: unknown mixin \"nope\""),
        ("@function f() { a { b: c } } $x: f();", "t.scss:1:17: CSS is not allowed inside a function\n  in function f (t.scss:1:34)"),
        ("@function f() { $a: 1; } $x: f();", "t.scss:1:30: function f did not return a value"),
        ("@function f($a) { @return $a; } $x: f();", "t.scss:1:37: function f: missing argument $a"),
        ("@function f($a) { @return $a; } $x: f(1, 2);", "t.scss:1:37: function f takes 1 argument, got 2"),
        ("@function floor($n) { @return 0; }", "t.scss:1:1: floor is a built-in function and cannot be redefined"),
        ("@function f() { @return 1; } @function f() { @return 2; }", "t.scss:1:30: function f is already defined in this scope"),
        ("@while true {}", "t.scss:1:1: evaluation budget exceeded (infinite loop?)"),
        ("@function f($n) { @return f($n); } $x: f(1);", "t.scss:1:27: function f: call depth exceeds 1000"),
        ("a { b: \"unterminated }", "t.scss:1:8: unterminated string"),
        ("a { b: c", "t.scss:1:3: unterminated block"),
        ("b: c;", "t.scss:1:1: a declaration must be inside a rule"),
        ("a { @return 1; }", "t.scss:1:5: @return is only allowed inside a function"),
        ("a { @content; }", "t.scss:1:5: @content is only allowed inside a mixin"),
        ("@use \"x.scss\"; a { }", "t.scss:1:1: cannot load \"x.scss\": no such file"),
        ("a { @use \"x.scss\"; }", "t.scss:1:5: @use must be at the top level of a file"),
        ("@error \"custom #{1 + 1}\";", "t.scss:1:1: custom 2"),
        ("@for $i from 1.5 to 3 {}", "t.scss:1:14: @for bound must be an integer, got 1.5"),
        ("a { b: nth(a b, 3) }", "t.scss:1:8: nth(): index 3 is out of range for 2 items"),
        ("$m: (a: 1); a { b: $m }", "t.scss:1:20: (a: 1) is not a CSS value"),
        ("a { *zoom: 1 }", "t.scss:1:5: invalid declaration name \"*zoom\""),
        ("$x: 1; $x: 2 !global;", "t.scss:1:14: !global is not needed"),
        ("@function f($a) { @return 1; } $x: f($a: 1);", "t.scss:1:38: keyword arguments are not supported"),
    ];
    for (input, want) in cases {
        match compile(input) {
            Ok(_) => panic!("{input}\n  expected error {want:?}"),
            Err(e) => {
                let got = e.to_string();
                assert!(got.starts_with(want), "{input}\n  got:  {got}\n  want: {want}");
            }
        }
    }
    let deep = format!("a {{ b: {}", "(".repeat(600));
    let got = compile(&deep).unwrap_err().to_string();
    assert!(got.starts_with("t.scss:1:507: nesting deeper than 500 levels"), "{got}");
}

#[test]
fn use_files() {
    let files: &[(&str, &str)] = &[
        ("site/lib/tokens.scss", "$accent: red !default;\n$used: 0;\n:root { --accent: #{$accent}; }\n@mixin box { border: 1px solid; }\n"),
        ("site/lib/twice.scss", "@use \"tokens.scss\";\n$used: $used + 1;\n"),
        ("site/lib/cycle.scss", "@use \"cycle.scss\";\n"),
    ];
    let load = |path: &str| -> Result<String, String> {
        files.iter().find(|(p, _)| *p == path).map(|(_, s)| s.to_string()).ok_or_else(|| "no such file".into())
    };
    let got = lace::Compiler::new()
        .load(Box::new(load))
        .log(Box::new(std::io::sink()))
        .compile("$accent: blue;\n@use \"lib/tokens.scss\";\n@use \"lib/twice.scss\";\n@use \"lib/tokens.scss\";\na { @include box; n: $used }\n", "site/main.scss")
        .unwrap();
    assert_eq!(got, ":root {\n  --accent: blue;\n}\n\na {\n  border: 1px solid;\n  n: 1;\n}\n");
    let err = lace::Compiler::new()
        .load(Box::new(load))
        .log(Box::new(std::io::sink()))
        .compile("@use \"lib/cycle.scss\";", "site/main.scss")
        .unwrap_err();
    assert!(err.to_string().contains("cycle"), "{err}");
}

#[test]
fn debug_output() {
    use std::sync::{Arc, Mutex};
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let buf = Arc::new(Mutex::new(Vec::new()));
    lace::Compiler::new().log(Box::new(Sink(buf.clone()))).compile("@debug (a: 1) \"s\";", "t.scss").unwrap();
    assert_eq!(String::from_utf8(buf.lock().unwrap().clone()).unwrap(), "t.scss:1:1: debug: (a: 1) \"s\"\n");
}
