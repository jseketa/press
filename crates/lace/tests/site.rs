//! The acceptance test: the author's real stylesheet must compile to what his
//! previous preprocessor produced, modulo whitespace. That file is plain
//! modern CSS plus // comments and two interpolated constants, which is
//! exactly the "CSS in, CSS out" invariant. (One block of the fixture is the
//! correct expansion of the stylesheet's @for loop, which the old
//! preprocessor could not do.)

use std::fs;

/// One statement per line, insignificant spacing dropped, the spaces that
/// matter inside selectors and values kept.
fn normalize(s: &str) -> String {
    let mut s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    for p in ["{", "}", ";", ","] {
        s = s.replace(&format!(" {p}"), p).replace(&format!("{p} "), p);
    }
    s = s.replace(": ", ":");
    for p in ["{", "}", ";"] {
        s = s.replace(p, &format!("{p}\n"));
    }
    s.trim().to_string()
}

#[test]
fn site() {
    let got = lace::compile_file("testdata/site/main.scss").unwrap();
    let want = fs::read_to_string("testdata/site/expected.css").unwrap();
    let (g, w) = (normalize(&got), normalize(&want));
    if g == w {
        return;
    }
    let (gl, wl): (Vec<_>, Vec<_>) = (g.lines().collect(), w.lines().collect());
    for (i, (a, b)) in gl.iter().zip(&wl).enumerate() {
        assert_eq!(a, b, "statement {} differs", i + 1);
    }
    panic!("statement count differs: got {}, want {}", gl.len(), wl.len());
}
