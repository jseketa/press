//! `press serve`: build, serve the output, rebuild whenever a source file
//! changes, and tell open pages to reload.
//!
//! Change detection is a poll: every 300 ms the source tree is walked and a
//! fingerprint of every file's mtime and size compared with the last one. A
//! walk takes about a millisecond and a rebuild about a hundred, so there is
//! nothing for a filesystem-notification dependency to improve. The HTTP
//! server is the hundred lines a file server and an event stream need; it
//! listens on localhost only, and still treats every request as hostile.

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
    let out = opts.out.canonicalize().map_err(|e| Error::new(opts.out.to_string_lossy(), e.to_string()))?;
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr).map_err(|e| Error::new(&addr, e.to_string()))?;
    println!("serving http://{addr}/ (rebuilds on change, reloads open pages)");

    let gen = Arc::new(Generation { n: Mutex::new(0), changed: Condvar::new() });
    let opts = Arc::new(opts);
    let out = Arc::new(out);
    {
        let (gen, opts) = (gen.clone(), opts.clone());
        thread::spawn(move || watch(&opts, &gen));
    }
    for stream in listener.incoming().flatten() {
        let (gen, out) = (gen.clone(), out.clone());
        thread::spawn(move || {
            let _ = handle(stream, &out, &gen);
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

/// A hash of the path, mtime and size of every source file, in the site
/// and in a theme directory outside it. The output and cache directories
/// are the build's own writes, and never a reason to build again.
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
                if skip.contains(&canon) || name == "node_modules" || name == "public" || name.starts_with('.') {
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
    if !opts.templates.starts_with(&opts.root) {
        walk(&opts.templates, &skip, &mut h);
    }
    h
}

/// Reloads on a rebuild, and also when the connection comes back after
/// press itself was restarted.
const RELOAD_SCRIPT: &str = "<script>(()=>{let dropped=false;const es=new EventSource(\"/_reload\");es.onmessage=()=>location.reload();es.onerror=()=>{dropped=true};es.onopen=()=>{if(dropped)location.reload()}})()</script>";

const MAX_LINE: u64 = 8192;
const MAX_HEADERS: usize = 100;

fn handle(mut stream: TcpStream, out: &Path, gen: &Generation) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    (&mut reader).take(MAX_LINE).read_line(&mut line)?;
    if !line.ends_with('\n') {
        return respond(&mut stream, "400 Bad Request", "text/plain; charset=utf-8", "", b"bad request\n", false);
    }
    let mut words = line.split_whitespace();
    let method = words.next().unwrap_or("");
    let target = words.next().unwrap_or("/");
    // Drain the headers; nothing in them matters to a file server.
    for _ in 0..MAX_HEADERS {
        let mut h = String::new();
        if (&mut reader).take(MAX_LINE).read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
    }
    let head_only = match method {
        "GET" => false,
        "HEAD" => true,
        _ => return respond(&mut stream, "405 Method Not Allowed", "text/plain; charset=utf-8", "Allow: GET, HEAD\r\n", b"method not allowed\n", false),
    };
    let path = target.split('?').next().unwrap_or("/");
    if path == "/_reload" {
        return event_stream(stream, gen);
    }

    // Decode, then resolve, then check: the served file must be inside the
    // output directory whatever the request spelled.
    let decoded = percent_decode(path);
    let mut file = out.to_path_buf();
    for seg in decoded.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                file.pop();
            }
            s if s.contains(['\\', ':']) => return not_found(&mut stream, head_only),
            s => file.push(s),
        }
    }
    let Ok(resolved) = file.canonicalize() else { return not_found(&mut stream, head_only) };
    if !resolved.starts_with(out) {
        return not_found(&mut stream, head_only);
    }
    let mut file = resolved;
    if file.is_dir() {
        if !path.ends_with('/') {
            let location = format!("Location: {}/\r\n", path);
            return respond(&mut stream, "301 Moved Permanently", "text/html; charset=utf-8", &location, b"", head_only);
        }
        file.push("index.html");
    }
    let Ok(mut body) = std::fs::read(&file) else { return not_found(&mut stream, head_only) };
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
        return respond(&mut stream, "200 OK", ctype, "Cache-Control: no-store\r\n", &body, head_only);
    }
    respond(&mut stream, "200 OK", ctype, "", &body, head_only)
}

fn not_found(stream: &mut TcpStream, head_only: bool) -> std::io::Result<()> {
    respond(stream, "404 Not Found", "text/plain; charset=utf-8", "", b"not found\n", head_only)
}

fn respond(stream: &mut TcpStream, status: &str, ctype: &str, extra: &str, body: &[u8], head_only: bool) -> std::io::Result<()> {
    stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n", body.len()).as_bytes())?;
    if !head_only {
        stream.write_all(body)?;
    }
    Ok(())
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

/// %XX on bytes, so a request cannot make us slice inside a character.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
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
