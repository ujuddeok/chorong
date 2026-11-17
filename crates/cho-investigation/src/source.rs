//! Where an investigation comes from, and reading it safely.
//!
//! A [`Source`] is a local file or an `https://` URL. Both are untrusted
//! (`design/investigations-viewer.md#security`): files are read with Chorong's safe
//! reads (regular files only, size-capped, nothing outside the repository when a
//! root is given), and fetches are bounded (https only, [`MAX_INVESTIGATION_BYTES`],
//! [`FETCH_TIMEOUT`], no credentials or cookies, redirects followed by hand and
//! only to `https://`).

use crate::{FormatError, Investigation};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// The largest investigation Chorong reads or fetches. Real files are a few kilobytes; a
/// large one is a mistake or an attack.
pub const MAX_INVESTIGATION_BYTES: u64 = 4 * 1024 * 1024;
/// How long a whole fetch, redirects included, may take.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
/// How many redirects a fetch follows.
pub const MAX_REDIRECTS: u32 = 5;
/// The longest URL accepted as a source.
pub const MAX_URL_LEN: usize = 2048;

/// Where an investigation is read from. Serialises as `{"path": "…"}` or `{"url": "…"}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// A local file. Absolute when it came from a link or from recents.
    Path(PathBuf),
    /// An `https://` URL without credentials.
    Url(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Read(String),
    /// The host could not be reached, or did not answer in time.
    #[error("{0}")]
    Fetch(String),
    #[error("{0}")]
    TooLarge(String),
    #[error("{0}")]
    Format(#[from] FormatError),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Path(p) => write!(f, "{}", p.display()),
            Source::Url(u) => f.write_str(u),
        }
    }
}

impl Source {
    /// The `src` of a `chorong://investigation?src=…` link: an `https://` URL or an absolute
    /// path with no `.` or `..` components.
    pub fn from_link_src(src: &str) -> Result<Self, SourceError> {
        if src.contains("://") {
            return Self::url(src);
        }
        if src.chars().any(char::is_control) {
            return Err(SourceError::Invalid(
                "the investigation path contains control characters".into(),
            ));
        }
        let path = Path::new(src);
        if !path.is_absolute() {
            return Err(SourceError::Invalid(format!(
                "an investigation link needs an absolute path or an https URL, got `{}`",
                src.escape_debug()
            )));
        }
        if path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        {
            return Err(SourceError::Invalid(format!(
                "the investigation path `{}` must not contain `.` or `..`",
                src.escape_debug()
            )));
        }
        Ok(Source::Path(path.to_path_buf()))
    }

    /// A command-line argument: an `https://` URL, or a path (relative to the current
    /// directory if not absolute). Other schemes are refused.
    pub fn from_arg(arg: &str) -> Result<Self, SourceError> {
        if arg.contains("://") {
            Self::url(arg)
        } else {
            Ok(Source::Path(PathBuf::from(arg)))
        }
    }

    /// Validates an `https://` URL: no credentials, a host, at most [`MAX_URL_LEN`].
    pub fn url(s: &str) -> Result<Self, SourceError> {
        check_url(s, false).map(|u| Source::Url(u.to_string()))
    }

    /// The host to show for a URL source (`None` for a file).
    pub fn host(&self) -> Option<String> {
        match self {
            Source::Path(_) => None,
            Source::Url(u) => url::Url::parse(u)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string)),
        }
    }

    /// Whether [`crate::session::InvestigationSession::check_for_update`] can watch it.
    /// Only files are re-read; a URL is fetched again only when opened again.
    pub fn is_file(&self) -> bool {
        matches!(self, Source::Path(_))
    }
}

/// Parses and checks a URL. `allow_loopback_http` is for tests, which serve plain HTTP on
/// 127.0.0.1; production code always passes `false`.
pub(crate) fn check_url(s: &str, allow_loopback_http: bool) -> Result<url::Url, SourceError> {
    let invalid = |why: &str| {
        Err(SourceError::Invalid(format!(
            "refusing investigation URL `{}`: {why}",
            truncate(s, 200).escape_debug()
        )))
    };
    if s.len() > MAX_URL_LEN {
        return invalid("too long");
    }
    if s.chars().any(char::is_control) {
        return invalid("contains control characters");
    }
    let Ok(u) = url::Url::parse(s) else {
        return invalid("not a valid URL");
    };
    let loopback = allow_loopback_http
        && u.scheme() == "http"
        && matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    if u.scheme() != "https" && !loopback {
        return invalid("only https:// is allowed");
    }
    if !u.username().is_empty() || u.password().is_some() {
        return invalid("URLs with credentials are not allowed");
    }
    if u.host_str().is_none_or(str::is_empty) {
        return invalid("no host");
    }
    Ok(u)
}

fn truncate(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Resolves a `chorong://investigation?src=…` link to its [`Source`].
pub fn source_from_link(link: &str) -> Result<Source, SourceError> {
    match cho_core::ChorongLink::parse(link) {
        Ok(cho_core::ChorongLink::Investigation { src }) => Source::from_link_src(&src),
        Ok(_) => Err(SourceError::Invalid(
            "not a `chorong://investigation` link".into(),
        )),
        Err(e) => Err(SourceError::Invalid(e.to_string())),
    }
}

/// The identity of a file's content: its length and a stable 64-bit hash (FNV-1a), so a
/// rewrite with the same modification time is still noticed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentState {
    pub len: u64,
    pub hash: u64,
}

impl ContentState {
    pub fn of(bytes: &[u8]) -> Self {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        Self {
            len: bytes.len() as u64,
            hash,
        }
    }
}

/// Raw bytes read from a source, before parsing.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub bytes: Vec<u8>,
    pub state: ContentState,
    /// For a URL: where it was fetched from after redirects (see [`host_of`]).
    pub final_url: Option<String>,
}

/// The host of a URL, e.g. a [`Fetched::final_url`].
pub fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
}

#[cfg(feature = "fetch")]
fn fetch_url(url: &str) -> Result<(Vec<u8>, String), SourceError> {
    crate::fetch::fetch(url, &crate::fetch::FetchPolicy::default())
}

#[cfg(not(feature = "fetch"))]
fn fetch_url(_url: &str) -> Result<(Vec<u8>, String), SourceError> {
    Err(SourceError::Fetch(
        "this build of Chorong can't fetch URLs (cho-investigation's `fetch` feature is off)"
            .into(),
    ))
}

/// Reads a source's bytes. A path is read with [`cho_core::fs::read_source_in`] when
/// `root` is given and contains it (so a symbolic link can't lead out of the
/// repository), otherwise with [`cho_core::fs::read_source`]. Either way the result is
/// at most [`MAX_INVESTIGATION_BYTES`].
pub fn read(source: &Source, root: Option<&Path>) -> Result<Fetched, SourceError> {
    let bytes = match source {
        Source::Path(p) => read_file(p, root)?,
        Source::Url(u) => {
            let (bytes, final_url) = fetch_url(u)?;
            return Ok(Fetched {
                state: ContentState::of(&bytes),
                bytes,
                final_url: Some(final_url),
            });
        }
    };
    Ok(Fetched {
        state: ContentState::of(&bytes),
        bytes,
        final_url: None,
    })
}

fn read_file(path: &Path, root: Option<&Path>) -> Result<Vec<u8>, SourceError> {
    let err = |e: cho_core::fs::ReadError| {
        SourceError::Read(format!("cannot read {}: {e}", path.display()))
    };
    // Size first, so a 16 MB file isn't read only to be refused.
    #[allow(clippy::disallowed_methods)] // metadata only; the read below is the safe one
    if let Ok(meta) = std::fs::metadata(path)
        && meta.is_file()
        && meta.len() > MAX_INVESTIGATION_BYTES
    {
        return Err(too_large(&path.display().to_string(), meta.len()));
    }
    let bytes = match root {
        Some(root) if path.is_relative() || path.starts_with(root) => {
            cho_core::fs::read_source_in(root, path).map_err(err)?
        }
        _ => cho_core::fs::read_source(path).map_err(err)?,
    };
    if bytes.len() as u64 > MAX_INVESTIGATION_BYTES {
        return Err(too_large(&path.display().to_string(), bytes.len() as u64));
    }
    Ok(bytes)
}

fn too_large(what: &str, len: u64) -> SourceError {
    SourceError::TooLarge(format!(
        "{what} is too large for an investigation ({len} bytes; the limit is {} MB)",
        MAX_INVESTIGATION_BYTES / (1024 * 1024)
    ))
}

/// Parses bytes as an investigation (parse, then validate).
pub fn parse_bytes(bytes: &[u8]) -> Result<Investigation, SourceError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| SourceError::Format(FormatError::Json("not UTF-8 text".into())))?;
    Ok(Investigation::parse(text)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_rules() {
        assert!(Source::url("https://example.com/a.json").is_ok());
        for bad in [
            "http://example.com/a.json",
            "file:///etc/passwd",
            "ftp://example.com/a",
            "https://user:pw@example.com/a",
            "https://user@example.com/a",
            "https://exa\nmple.com/a",
            "https://",
        ] {
            assert!(Source::url(bad).is_err(), "{bad}");
        }
        assert!(Source::url(&format!("https://e.com/{}", "a".repeat(MAX_URL_LEN))).is_err());
        assert_eq!(
            Source::url("https://Example.com/x")
                .unwrap()
                .host()
                .as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn link_sources() {
        let ok = |l: &str| source_from_link(l).unwrap();
        assert_eq!(
            ok("chorong://investigation?src=https://example.com/t.json"),
            Source::Url("https://example.com/t.json".into())
        );
        assert_eq!(
            ok("chorong://investigation?src=/home/a/t.json"),
            Source::Path("/home/a/t.json".into())
        );
        assert_eq!(
            ok("chorong://investigation?src=%2Fhome%2Fa%2Ft.json"),
            Source::Path("/home/a/t.json".into())
        );
        for bad in [
            "chorong://investigation?src=relative/t.json",
            "chorong://investigation?src=/home/a/../b.json",
            "chorong://investigation?src=http://example.com/t.json",
            "chorong://investigation?src=file:///etc/passwd",
            "chorong://investigation",
            "chorong://open?repo=github:a/b",
            "https://example.com",
        ] {
            assert!(source_from_link(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn content_state_is_stable() {
        assert_eq!(
            ContentState::of(b"abc"),
            ContentState {
                len: 3,
                hash: 0xe71f_a219_0541_574b
            }
        );
        assert_ne!(ContentState::of(b"abc"), ContentState::of(b"abd"));
    }

    #[test]
    fn file_size_cap() {
        let dir = std::env::temp_dir().join(format!("cho-src-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.json");
        std::fs::write(&big, vec![b' '; MAX_INVESTIGATION_BYTES as usize + 1]).unwrap();
        assert!(matches!(
            read(&Source::Path(big), None),
            Err(SourceError::TooLarge(_))
        ));
        std::fs::remove_dir_all(dir).ok();
    }
}
