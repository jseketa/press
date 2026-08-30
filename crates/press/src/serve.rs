//! `press serve`: build, serve the output, rebuild whenever a source file
//! changes, and tell open pages to reload.
//!
//! Change detection is a poll: every 300 ms the source tree is walked and a
//! fingerprint of every file's mtime and size compared with the last one. A
//! walk takes about a millisecond and a rebuild about a hundred, so there is
//! nothing for a filesystem-notification dependency to improve. The HTTP
//! server is the sixty lines a file server and an event stream need.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::site::{build, Options};

/// The rebuild generation, bumped after every successful build; event-stream
/// connections wait on it.
struct Generation {
    n: Mutex<u64>,
    changed: Condvar,
}

pub fn serve(opts: Options, port: u16) -> Result<()> {
    let stats = build(&opts)?;
    println!("{} pages -> {} in {}ms", stats.pages, opts.out.display(), stats.millis);
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr).map_err(|e| Error::new(&addr, e.to_string()))?;
    println!("serving http://{addr}/ (rebuilds on change, reloads open pages)");

    let gen = Arc::new(Generation { n: Mutex::new(0), changed: Condvar::new() });
    let opts = Arc::new(opts);
    {
        let (gen, opts) = (gen.clone(), opts.clone());
        thread::spawn(move || watch(&opts, &gen));
    }
    for stream in listener.incoming().flatten() {
        let (gen, opts) = (gen.clone(), opts.clone());
        thread::spawn(move || {
            let _ = handle(stream, &opts.out, &gen);
        });
    }
    Ok(())
}

fn watch(opts: &Options, gen: &Generation) {
    let mut last = fingerprint(opts);
    loop {
        thread::sleep(Duration::from_millis(300));
        let now = fingerprint(opts);
        if now == last {
            continue;
        }
        last = now;
        match build(opts) {
            Ok(stats) => {
                println!("{} pages -> {} in {}ms", stats.pages, opts.out.display(), stats.millis);
                *gen.n.lock().unwrap() += 1;
                gen.changed.notify_all();
            }
            // The open page keeps what it has; the next save gets another go.
            Err(e) => println!("build failed: {e}"),
        }
    }
}

/// A hash of the path, mtime and size of every source file. The output and
/// cache directories are the build's own writes, and never a reason to
/// build again.
fn fingerprint(opts: &Options) -> u64 {
    let skip: Vec<PathBuf> = [&opts.out, &opts.cache_dir].iter().filter_map(|p| p.canonicalize().ok()).collect();
    let mut h: u64 = 0xcbf29ce484222325;
    fn walk(dir: &Path, skip: &[PathBuf], h: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                let canon = p.canonicalize().unwrap_or_default();
                // public is Zola's output directory and node_modules is npx's.
                if skip.contains(&canon) || name == ".git" || name == "node_modules" || name == "public" || name.starts_with('.') {
                    continue;
                }
                walk(&p, skip, h);
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
            for b in p.to_string_lossy().bytes().chain(mtime.to_le_bytes()).chain(meta.len().to_le_bytes()) {
                *h ^= b as u64;
                *h = h.wrapping_mul(0x100000001b3);
            }
        }
    }
    walk(&opts.root, &skip, &mut h);
    h
}

/// Reloads on a rebuild, and also when the connection comes back after
/// press itself was restarted.
const RELOAD_SCRIPT: &str = "<script>(()=>{let dropped=false;const es=new EventSource(\"/_reload\");es.onmessage=()=>location.reload();es.onerror=()=>{dropped=true};es.onopen=()=>{if(dropped)location.reload()}})()</script>";

fn handle(mut stream: TcpStream, out: &Path, gen: &Generation) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let path = line.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap_or("/").to_string();
    // Drain the headers; nothing in them matters to a file server.
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
    }

    if path == "/_reload" {
        return event_stream(stream, gen);
    }

    let mut clean = String::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if let Some(i) = clean.rfind('/') {
                    clean.truncate(i);
                }
            }
            s => {
                clean.push('/');
                clean.push_str(&percent_decode(s));
            }
        }
    }
    let mut file = out.to_path_buf();
    for seg in clean.split('/').filter(|s| !s.is_empty()) {
        file.push(seg);
    }
    if file.is_dir() {
        if !path.ends_with('/') {
            return respond(&mut stream, "301 Moved Permanently", "text/html", format!("Location: {clean}/\r\n").as_bytes(), b"");
        }
        file.push("index.html");
    }
    let Ok(mut body) = std::fs::read(&file) else {
        return respond(&mut stream, "404 Not Found", "text/plain; charset=utf-8", b"", b"not found\n");
    };
    let ctype = content_type(&file);
    if ctype.starts_with("text/html") {
        // The reload script is added at serve time, so the built files stay
        // exactly what a deploy would publish.
        let mut html = String::from_utf8_lossy(&body).into_owned();
        match html.rfind("</body>") {
            Some(i) => html.insert_str(i, RELOAD_SCRIPT),
            None => html.push_str(RELOAD_SCRIPT),
        }
        body = html.into_bytes();
        return respond(&mut stream, "200 OK", ctype, b"Cache-Control: no-store\r\n", &body);
    }
    respond(&mut stream, "200 OK", ctype, b"", &body)
}

fn respond(stream: &mut TcpStream, status: &str, ctype: &str, extra: &[u8], body: &[u8]) -> std::io::Result<()> {
    stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len()).as_bytes())?;
    stream.write_all(extra)?;
    stream.write_all(b"\r\n")?;
    stream.write_all(body)
}

/// One "reload" event per rebuild, for as long as the browser listens.
fn event_stream(mut stream: TcpStream, gen: &Generation) -> std::io::Result<()> {
    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n: ready\n\n")?;
    let mut seen = *gen.n.lock().unwrap();
    loop {
        let guard = gen.n.lock().unwrap();
        let (guard, _) = gen.changed.wait_timeout(guard, Duration::from_secs(15)).unwrap();
        let now = *guard;
        drop(guard);
        if now != seen {
            seen = now;
            stream.write_all(b"data: reload\n\n")?;
        } else {
            // A comment keeps the connection alive and detects a closed one.
            stream.write_all(b": ping\n\n")?;
        }
        stream.flush()?;
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 + 0 {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn content_type(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[allow(dead_code)]
fn _read<R: Read>(_: R) {}
