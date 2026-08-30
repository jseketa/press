//! The lace command.
//!
//!     lace input.scss              CSS on stdout
//!     lace input.scss -o out.css   flags may appear anywhere
//!     lace < input.scss            stdin; @use resolves against the cwd
//!     lace -prelude                print the standard library
//!     lace -budget N input.scss    allow N evaluation steps (-1: unlimited)

use std::io::{Read, Write};
use std::process::exit;

fn usage() -> ! {
    eprintln!("usage: lace [-o out.css] [-budget N] [input.scss]\n       lace -prelude");
    exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out: Option<String> = None;
    let mut input: Option<String> = None;
    let mut budget = lace::DEFAULT_BUDGET;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-prelude" | "--prelude" => {
                print!("{}", lace::prelude());
                return;
            }
            "-o" | "--o" => {
                i += 1;
                out = Some(args.get(i).cloned().unwrap_or_else(|| usage()));
            }
            "-budget" | "--budget" => {
                i += 1;
                budget = args.get(i).and_then(|s| s.parse().ok()).unwrap_or_else(|| usage());
            }
            "-h" | "-help" | "--help" => usage(),
            a => {
                if input.is_some() || (a.len() > 1 && a.starts_with('-')) {
                    usage();
                }
                input = Some(a.to_string());
            }
        }
        i += 1;
    }

    let (src, name) = match &input {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(s) => (s, path.replace('\\', "/")),
            Err(e) => {
                eprintln!("{path}: {e}");
                exit(1);
            }
        },
        None => {
            let mut s = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut s) {
                eprintln!("stdin: {e}");
                exit(1);
            }
            (s, "stdin".to_string())
        }
    };

    let css = match lace::Compiler::new().budget(budget).compile(&src, &name) {
        Ok(css) => css,
        Err(e) => {
            eprintln!("{e}");
            exit(1);
        }
    };
    let result = match out {
        Some(path) => std::fs::write(&path, css).map_err(|e| format!("{path}: {e}")),
        None => std::io::stdout().write_all(css.as_bytes()).map_err(|e| e.to_string()),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        exit(1);
    }
}
