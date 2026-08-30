//! Command press builds a site.
//!
//!     press -site ../jseketa.github.io          build once
//!     press -site ../jseketa.github.io serve    build, serve, rebuild on
//!                                               change, reload open pages

mod blocks;
mod config;
mod content;
mod error;
mod highlight;
mod markdown;
mod serve;
mod site;
mod template;

use std::path::PathBuf;
use std::process::exit;

fn usage() -> ! {
    eprintln!("usage: press [-site DIR] [-out DIR] [-templates DIR] [-cache DIR] [-port N] [serve]");
    exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut root = ".".to_string();
    let (mut out, mut templates, mut cache) = (None, None, None);
    let mut port: u16 = 1112;
    let mut serve_mode = false;
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
            _ => usage(),
        }
        i += 1;
    }

    let root = std::fs::canonicalize(&root).unwrap_or_else(|e| {
        eprintln!("{root}: {e}");
        exit(1)
    });
    if !root.join("config.toml").exists() {
        eprintln!("no config.toml in {}", root.display());
        exit(1);
    }
    let opts = site::Options {
        out: out.map_or_else(|| root.join("public-press"), PathBuf::from),
        templates: templates.map_or_else(|| root.join("theme"), PathBuf::from),
        cache_dir: cache.map_or_else(|| root.join(".press-cache"), PathBuf::from),
        root,
    };

    let result = if serve_mode {
        serve::serve(opts, port)
    } else {
        site::build(&opts).map(|stats| println!("{} pages -> {} in {}ms", stats.pages, opts.out.display(), stats.millis))
    };
    if let Err(e) = result {
        eprintln!("{e}");
        exit(1);
    }
}
