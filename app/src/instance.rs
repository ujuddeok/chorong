//! One running viewer per user on Linux: a newly launched `chorong-app` given a `chorong://` link
//! or a path hands it to the running viewer over a per-user local socket and exits; with none running
//! it opens normally and starts listening. macOS needs none of this: the OS delivers links
//! to the running app (`on_open_urls` in `main.rs`). See `docs/public/url-scheme.md`.
//!
//! The socket is `$XDG_RUNTIME_DIR/chorong/viewer.sock`, or `/tmp/chorong-UID/viewer.sock`; the
//! folder is created private to the user (0700), and neither side uses it unless it is owned by
//! the user and closed to others (another user could create `/tmp/chorong-UID` first). `CHORONG_SOCKET`
//! names another path (tests, screenshots). The protocol is one line: the link, an absolute path, or several absolute paths
//! joined by tabs, answered by `ok`.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Longest link or path accepted over the socket.
const MAX_LINE: u64 = 64 * 1024;

pub fn socket_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CHORONG_SOCKET").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let dir = match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d).join("chorong"),
        None => {
            let uid = std::fs::metadata("/proc/self").ok()?.uid();
            std::env::temp_dir().join(format!("chorong-{uid}"))
        }
    };
    Some(dir.join("viewer.sock"))
}

/// Sends `link` to a running viewer. True when one took it.
pub fn hand_over(link: &str) -> bool {
    socket_path().is_some_and(|p| hand_over_at(&p, link, !socket_overridden()))
}

/// True when the socket's folder belongs to this user and no one else can enter it.
fn folder_is_private(path: &Path) -> bool {
    let (Some(dir), Ok(me)) = (path.parent(), std::fs::metadata("/proc/self")) else {
        return false;
    };
    std::fs::symlink_metadata(dir)
        .is_ok_and(|m| m.is_dir() && m.uid() == me.uid() && m.mode() & 0o077 == 0)
}

/// `CHORONG_SOCKET` was set: the caller chose the path, so its folder is not checked.
fn socket_overridden() -> bool {
    std::env::var_os("CHORONG_SOCKET").is_some_and(|v| !v.is_empty())
}

fn hand_over_at(path: &Path, link: &str, check: bool) -> bool {
    if check && !folder_is_private(path) {
        return false;
    }
    let Ok(mut stream) = UnixStream::connect(path) else {
        return false;
    };
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
    if writeln!(stream, "{}", link.trim()).is_err() {
        return false;
    }
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).is_ok() && reply.trim() == "ok"
}

/// Starts listening for links from later launches; each one is passed to `deliver` on a
/// background thread. Does nothing when another viewer already listens.
pub fn listen(deliver: impl Fn(String) + Send + 'static) -> std::io::Result<()> {
    let path = socket_path().ok_or_else(|| std::io::Error::other("no socket path"))?;
    listen_at(&path, !socket_overridden(), deliver)
}

fn listen_at(
    path: &Path,
    check: bool,
    deliver: impl Fn(String) + Send + 'static,
) -> std::io::Result<()> {
    if UnixStream::connect(path).is_ok() {
        return Err(std::io::Error::other("another viewer is listening"));
    }
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    if check && !folder_is_private(path) {
        return Err(std::io::Error::other(format!(
            "{} is not private to this user (owner or permissions); not listening",
            path.parent().unwrap_or(path).display()
        )));
    }
    // A socket left by a viewer that exited without cleaning up.
    std::fs::remove_file(path).ok();
    let listener = UnixListener::bind(path)?;
    std::thread::Builder::new()
        .name("cho-links".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                let read = BufReader::new((&stream).take(MAX_LINE)).read_line(&mut line);
                let link = line.trim().to_string();
                if read.is_ok() && (link.starts_with("chorong://") || link.starts_with('/')) {
                    deliver(link);
                    let _ = (&stream).write_all(b"ok\n");
                } else {
                    let _ = (&stream)
                        .write_all(b"error: expected a chorong:// link or an absolute path\n");
                }
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_second_launch_hands_its_link_to_the_listener() {
        let dir_guard = tempfile::tempdir().unwrap();
        // Not created yet: listen_at makes it, private.
        let dir = dir_guard.path().join("sock");
        let path = dir.join("viewer.sock");
        assert!(
            !hand_over_at(&path, "chorong://open?repo=local:/x", true),
            "nobody listening"
        );
        let (tx, rx) = mpsc::channel();
        listen_at(&path, true, move |l| tx.send(l).unwrap()).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o700, "the folder is private");
        assert!(listen_at(&path, true, |_| {}).is_err(), "only one listener");
        assert!(hand_over_at(
            &path,
            "chorong://open?repo=local:/x&path=a.rs",
            true
        ));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            "chorong://open?repo=local:/x&path=a.rs"
        );
        assert!(
            hand_over_at(&path, "/srv/project/src", true),
            "a plain path is accepted"
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            "/srv/project/src"
        );
        assert!(!hand_over_at(&path, "not a link", true));
        assert!(
            !hand_over_at(&path, "relative/path", true),
            "paths must be absolute"
        );
    }

    #[test]
    fn a_folder_open_to_others_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir_guard = tempfile::tempdir().unwrap();
        let dir = dir_guard.path().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        let path = dir.join("viewer.sock");
        assert!(
            listen_at(&path, true, |_| {}).is_err(),
            "must not listen there"
        );
        assert!(!hand_over_at(&path, "/x", true), "must not hand over there");
    }
}
