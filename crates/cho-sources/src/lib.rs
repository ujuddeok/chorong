//! Remote sources for Chorong: projects (remembered remote repositories), the repository cache
//! (one bare, blobless clone per repository and one detached worktree per branch, pull
//! request or commit opened), fetching, merge bases, pull-request details over REST, and host
//! tokens. No GPUI; the viewer and the CLI both use it.
//!
//! Chorong writes only inside its own cache. A user's own clones are never written to and never
//! used as an object source. See `design/sources.md`.
//!
//! The `git` command does the network work and the writes (clone, fetch, worktree add and
//! remove); gix does the reads ([`reads`]). Callers don't need to know which is used.

pub mod auth;
pub mod gitcmd;
pub mod hosts;
mod open;
pub mod paths;
pub mod prepare;
pub mod progress;
pub mod projects;
pub mod reads;
pub mod recent;
pub mod state;
pub mod target;
pub mod tools;
pub mod usage;

#[cfg(test)]
mod testutil;

pub use auth::{Credential, Host, Keychain, SecretStore, TokenSource, Tokens};
pub use gitcmd::Git;
pub use hosts::{Api, PrDetails, PrState, RateLimit};
pub use open::{
    BaseSource, LOCK_TIMEOUT, OpenRequest, Opened, PrHead, PrSnapshot, RemoteHeads, RemovedProject,
    Sources, url_host,
};
pub use paths::Cache;
pub use prepare::ops::{Permission, index_permission};
pub use progress::{Cancel, GitProgress, PrepareStage, Stage};
pub use projects::{Project, Projects, Protocol};
pub use reads::CommitInfo;
pub use state::OpenedAs;
pub use target::{Target, TargetKind, parse_repo, parse_target};

use std::fmt;
use std::path::Path;

/// What kind of failure an [`Error`] is, so a caller (the viewer's open form) can react: offer
/// to add a token, say it's offline, ask which of two things was meant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// The host or remote couldn't be reached (no network, refused connection, DNS).
    Offline,
    /// The host refused or wanted credentials; a token or SSH key for `host` may help.
    Auth {
        host: String,
    },
    /// No such repository, branch, pull request, commit or worktree.
    NotFound,
    /// No commit `commit` (as it was asked for, lower case) in the repository or on its host.
    CommitNotFound {
        commit: String,
    },
    /// `name` is both a branch and a commit id.
    Ambiguous {
        name: String,
    },
    /// Another Chorong holds the repository's lock.
    LockBusy,
    /// Something Chorong doesn't do (a URL kind, a fork's pull request on Bitbucket, an old git).
    Unsupported,
    /// The caller cancelled.
    Cancelled,
    Other,
}

/// An error: a short sentence for the user (`message`: the host or repository and the
/// cause), the full story for logs and a "Details" disclosure (`detail`: the git command,
/// its stderr, paths), and its [`ErrorKind`]. Neither ever holds a token or password: URL
/// credentials are redacted ([`redact`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    pub detail: Option<String>,
}

impl Error {
    /// An error of kind [`ErrorKind::Other`].
    pub fn msg(m: impl Into<String>) -> Self {
        Self::new(ErrorKind::Other, m)
    }

    pub fn new(kind: ErrorKind, m: impl Into<String>) -> Self {
        Self {
            kind,
            message: redact(&m.into()),
            detail: None,
        }
    }

    /// Adds the full detail (redacted).
    pub fn with_detail(mut self, d: impl Into<String>) -> Self {
        let d = redact(&d.into());
        self.detail = (!d.trim().is_empty()).then_some(d);
        self
    }

    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, m)
    }

    pub fn unsupported(m: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unsupported, m)
    }

    pub fn cancelled() -> Self {
        Self::new(ErrorKind::Cancelled, "cancelled")
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Removes credentials from text that may quote a URL: `scheme://user:secret@host` becomes
/// `scheme://***@host`.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        // The authority ends at the first `/`, whitespace or quote.
        let end = tail
            .find(|c: char| c == '/' || c.is_whitespace() || c == '\'' || c == '"')
            .unwrap_or(tail.len());
        let authority = &tail[..end];
        match authority.rfind('@') {
            Some(at) => {
                out.push_str("***");
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Writes `bytes` to `path` through a temporary file in the same folder and a rename, so a
/// reader never sees half a file. Creates the folder.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| Error::msg(format!("{} has no parent folder", path.display())))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| Error::msg(format!("cannot create {}: {e}", dir.display())))?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes)
        .and_then(|_| std::fs::rename(&tmp, path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            Error::msg(format!("cannot write {}: {e}", path.display()))
        })
}

/// Seconds since the Unix epoch.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn redacts_url_credentials() {
        assert_eq!(
            redact("fatal: unable to access 'https://user:tok@github.com/a/b.git/': x"),
            "fatal: unable to access 'https://***@github.com/a/b.git/': x"
        );
        assert_eq!(
            redact("http://h:1/x and ssh://git@h/y"),
            "http://h:1/x and ssh://***@h/y"
        );
        assert_eq!(redact("no urls here"), "no urls here");
        let e = Error::msg("x https://a:b@h/").with_detail("y https://c:d@h/z");
        assert_eq!(e.message, "x https://***@h/");
        assert_eq!(e.detail.as_deref(), Some("y https://***@h/z"));
        assert_eq!(Error::msg("m").with_detail("  ").detail, None);
    }
}
