//! Test helpers: a local HTTP stub for the REST calls and local bare repositories served
//! over `file://`. Nothing here touches the network.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A request the stub saw: path and `Authorization` header.
#[derive(Debug, Clone)]
pub struct Seen {
    pub path: String,
    pub auth: Option<String>,
}

/// Serves `routes` (path → status, body, extra headers) on 127.0.0.1 until dropped.
pub struct Stub {
    pub base: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
}

pub type Route = (&'static str, u16, String, Vec<(&'static str, &'static str)>);

impl Stub {
    pub fn start(routes: Vec<Route>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(vec![]));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let mut auth = None;
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':')
                        && k.eq_ignore_ascii_case("authorization")
                    {
                        auth = Some(v.trim().to_string());
                    }
                }
                seen2.lock().unwrap().push(Seen {
                    path: path.clone(),
                    auth,
                });
                let (status, body, headers) = routes
                    .iter()
                    .find(|r| r.0 == path)
                    .map(|r| (r.1, r.2.clone(), r.3.clone()))
                    .unwrap_or((404, "{}".into(), vec![]));
                let mut resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for (k, v) in headers {
                    resp.push_str(&format!("{k}: {v}\r\n"));
                }
                resp.push_str("\r\n");
                resp.push_str(&body);
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        Self { base, seen }
    }
}

pub use cho_core::test_repo::git;

/// A "remote": a bare repository at `<dir>/remote.git` that allows filtered and by-id
/// fetches, filled from a work repository at `<dir>/work`. Returns (work, remote).
pub fn remote(dir: &Path) -> (PathBuf, PathBuf) {
    let work = dir.join("work");
    let bare = dir.join("remote.git");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    git(
        dir,
        &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
    );
    for (k, v) in [
        ("uploadpack.allowFilter", "true"),
        ("uploadpack.allowAnySHA1InWant", "true"),
    ] {
        git(&bare, &["config", k, v]);
    }
    git(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
    (work, bare)
}

/// Writes `files` and commits them with `message`; returns the commit id.
pub fn commit(work: &Path, files: &[(&str, &str)], message: &str) -> String {
    for (p, c) in files {
        let path = work.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, c).unwrap();
    }
    git(work, &["add", "-A"]);
    git(work, &["commit", "-q", "--allow-empty", "-m", message]);
    git(work, &["rev-parse", "HEAD"])
}

pub fn file_url(p: &Path) -> String {
    format!("file://{}", p.display())
}

/// A local HTTP server that accepts connections and never answers: git and HTTP clients
/// talking to it block until cancelled or timed out. Returns its base URL.
pub fn hanging_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let mut held = vec![];
        for s in listener.incoming().flatten() {
            held.push(s);
        }
    });
    base
}

/// A local port with nothing listening on it.
pub fn closed_port_url() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    format!("http://127.0.0.1:{port}")
}
