//! Command press builds a site.
//!
//!     press -site ../jseketa.github.io          build once
//!     press -site ../jseketa.github.io serve    build, serve, rebuild on
//!                                               change, reload open pages
//!     press -site ../jseketa.github.io present talk.md -out talk.html
//!                                               one Markdown file as a
//!                                               self-contained talk

mod blocks;
mod config;
mod content;
mod error;
mod highlight;
mod markdown;
mod present;
mod serve;
mod site;
mod template;

use std::path::PathBuf;
use std::process::exit;

fn usage() -> ! {
    eprintln!("usage: press [-site DIR] [-out DIR|FILE] [-templates DIR] [-cache DIR] [-port N] [serve | present TALK.md]");
    exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut root = ".".to_string();
    let (mut out, mut templates, mut cache) = (None, None, None);
    let mut port: u16 = 1112;
    let mut serve_mode = false;
    let mut talk: Option<String> = None;
    let mut i = 0;
    let value = |i: &mut usize| -> String {
        *i += 1;
        args.get(*i).cloned().unwrap_or_else(|| usage())
    };
    while i < args.len() {
        match args[i].trim_start_matches('-') {
            "site" => root = value(&mut i),
            "out" => out = Some(value(&mut i)),
            "templates" => templates = Some(value(&mut i)),
            "cache" => cache = Some(value(&mut i)),
            "port" => port = value(&mut i).parse().unwrap_or_else(|_| usage()),
            "serve" if !args[i].starts_with('-') => serve_mode = true,
            "present" if !args[i].starts_with('-') => talk = Some(value(&mut i)),
            _ => usage(),
        }
        i += 1;
    }

    let root = std::fs::canonicalize(&root).unwrap_or_else(|e| {
        eprintln!("{root}: {e}");
        exit(1)
    });
    // Windows canonical paths carry a \\?\ prefix that reads badly in
    // messages and confuses tools; the plain form is what the user typed.
    let root = PathBuf::from(root.to_string_lossy().trim_start_matches(r"\\?\"));
    if !root.join("config.toml").exists() {
        eprintln!("no config.toml in {}", root.display());
        exit(1);
    }
    // For present, -out names the HTML file; the site's output directory is
    // not involved.
    let present_out = if talk.is_some() { out.take().map(PathBuf::from) } else { None };
    let opts = site::Options {
        out: out.map_or_else(|| root.join("public-press"), PathBuf::from),
        templates: templates.map_or_else(|| root.join("theme"), PathBuf::from),
        cache_dir: cache.map_or_else(|| root.join(".press-cache"), PathBuf::from),
        root,
    };

    let result = if let Some(talk) = talk {
        let talk = PathBuf::from(talk);
        if !talk.is_file() {
            eprintln!("{}: no such file", talk.display());
            exit(1);
        }
        match present_out {
            Some(file) => present::write(&opts, &talk, &file),
            None => present::serve(opts, talk, port),
        }
    } else if serve_mode {
        serve::serve(opts, port)
    } else {
        site::build(&opts).map(|stats| println!("{} pages -> {} in {}ms", stats.pages, opts.out.display(), stats.millis))
    };
    if let Err(e) = result {
        eprintln!("{e}");
        exit(1);
    }
}
