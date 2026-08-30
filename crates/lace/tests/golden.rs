//! The golden suite: every testdata/golden/NAME.scss compiles to NAME.css
//! exactly, or fails with the first line of NAME.err (and any frame lines
//! the .err lists must appear in the error).

use std::fs;
use std::path::Path;

fn loader(path: &str) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("{path}: file not found"),
        _ => e.to_string(),
    })
}

#[test]
fn golden() {
    let dir = Path::new("testdata/golden");
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |x| x == "scss"))
        .map(|e| e.path().file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert!(names.len() > 150, "found only {} cases", names.len());

    let mut failures = Vec::new();
    for name in &names {
        let scss = format!("testdata/golden/{name}.scss");
        let css = dir.join(format!("{name}.css"));
        let err = dir.join(format!("{name}.err"));
        assert!(css.exists() != err.exists(), "{name}: needs exactly one of .css/.err");
        let src = fs::read_to_string(&scss).unwrap();
        let result = lace::Compiler::new().load(Box::new(loader)).log(Box::new(std::io::sink())).compile(&src, &scss);
        match (result, css.exists()) {
            (Ok(got), true) => {
                let want = fs::read_to_string(&css).unwrap().replace("\r\n", "\n");
                if got != want {
                    failures.push(format!("{name}: output mismatch\n--- got ---\n{got}--- want ---\n{want}"));
                }
            }
            (Err(e), false) => {
                let want = fs::read_to_string(&err).unwrap().replace("\r\n", "\n");
                let mut lines = want.lines().filter(|l| !l.trim().is_empty());
                let first = lines.next().unwrap_or("");
                let text = e.to_string();
                let got_first = text.lines().next().unwrap_or("");
                if got_first != first {
                    failures.push(format!("{name}: error mismatch\n  got:  {got_first}\n  want: {first}"));
                } else if let Some(frame) = lines.find(|l| !text.lines().any(|t| t == *l)) {
                    failures.push(format!("{name}: missing frame line {frame:?} in:\n{text}"));
                }
            }
            (Ok(got), false) => failures.push(format!("{name}: expected an error, got output:\n{got}")),
            (Err(e), true) => failures.push(format!("{name}: unexpected error: {e}")),
        }
    }
    for f in &failures {
        eprintln!("{f}\n");
    }
    assert!(failures.is_empty(), "{} of {} golden cases failed", failures.len(), names.len());
}
