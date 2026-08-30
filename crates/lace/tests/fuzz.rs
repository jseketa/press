//! A small mutation fuzzer: every input must produce CSS or an Error, never
//! a panic, and must finish under the step budget. Not a replacement for a
//! coverage-guided fuzzer, but it runs on every `cargo test`.

use std::fs;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const ALPHABET: &[u8] = b"{}();:$#@\"'/*-+.,%[]!\\ \n\tabcxyz0123456789#{}url(if(&";

#[test]
fn mutations_never_panic() {
    let mut seeds: Vec<Vec<u8>> = vec![
        fs::read("testdata/site/main.scss").unwrap(),
        b"$g: 8px; a { p: $g * 2 ($g / 2); q: calc(100% - $g); &:hover { c: red } @media (x: #{$g}) { b: c } }".to_vec(),
        b"@mixin m($a, $r...) { .m { a: $a; r: $r; @content; } } @include m(1, 2, 3) { z: 1 }".to_vec(),
        b"@function f($n) { @if $n <= 1 { @return 1; } @return $n * f($n - 1); } a { b: f(6) }".to_vec(),
        b"@each $k, $v in (a: 1, b: 2,) { .#{$k} { v: $v; } } @for $i from 1 through 3 { .i-#{$i} { w: $i } }".to_vec(),
        b"$i: 0; @while $i < 3 { $i: $i + 1; } a { b: $i; c: \"s#{$i}\" + x; d: url(//x/y.png); --p: { a; } }".to_vec(),
        b"@layer base {} @property --a { syntax: \"<angle>\"; inherits: false; } a { b: if(style(--w: true): 1; else: 2) }".to_vec(),
        b"a { b: c // comment\n /* keep */ d: e }".to_vec(),
        b"@use \"nope.scss\";".to_vec(),
        b"a { b: [full-start] 1fr [full-end]; c: U+4??; d: 1e3 .5 -.5 +5 1E-3; e: \"q\\\"x\" 'y'; f: #{1 + 2}px }".to_vec(),
        b"@use\"\r".to_vec(),
    ];
    for f in fs::read_dir("testdata/golden").unwrap().flatten() {
        if f.path().extension().map_or(false, |x| x == "scss") {
            seeds.push(fs::read(f.path()).unwrap());
        }
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    for round in 0..20_000 {
        let mut input = seeds[rng.below(seeds.len())].clone();
        if input.len() > 2000 {
            let start = rng.below(input.len() - 2000);
            input = input[start..start + 2000].to_vec();
        }
        for _ in 0..1 + rng.below(4) {
            match rng.below(4) {
                0 if !input.is_empty() => {
                    let i = rng.below(input.len());
                    input.remove(i);
                }
                1 => {
                    let i = rng.below(input.len() + 1);
                    input.insert(i, ALPHABET[rng.below(ALPHABET.len())]);
                }
                2 if !input.is_empty() => {
                    let i = rng.below(input.len());
                    input[i] = ALPHABET[rng.below(ALPHABET.len())];
                }
                _ => {
                    let other = &seeds[rng.below(seeds.len())];
                    if !other.is_empty() {
                        let a = rng.below(other.len());
                        let b = (a + rng.below(64)).min(other.len());
                        let at = rng.below(input.len() + 1);
                        input.splice(at..at, other[a..b].iter().copied());
                    }
                }
            }
        }
        let src = String::from_utf8_lossy(&input).into_owned();
        let _ = lace::Compiler::new()
            .load(Box::new(|_| Err("no such file".into())))
            .log(Box::new(std::io::sink()))
            .budget(200_000)
            .compile(&src, &format!("fuzz{round}.scss"));
    }
}
