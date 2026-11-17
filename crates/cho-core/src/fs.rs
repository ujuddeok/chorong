//! Reading files from a repository the reader opened.
//!
//! Repository content is untrusted input. A link, an index, an investigation or a symbolic link
//! in the tree can name any path, so a plain `std::fs::read` could hang on a FIFO, read
//! `/dev/zero` until memory runs out, load a multi-gigabyte file, or follow a symlink to
//! `~/.ssh` and show it. Every read of repository content goes through [`read_source_in`]
//! (or [`read_source`] for a file the reader named themselves): regular files only, at most
//! [`MAX_SOURCE_BYTES`], and with a root, nothing that resolves outside it.
//!
//! `clippy.toml` disallows `std::fs::read` and `std::fs::read_to_string` workspace-wide so new
//! code cannot skip this by accident; reads of Chorong's own files (settings, caches, `/proc`)
//! carry `#[allow(clippy::disallowed_methods)]` with the reason.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The largest file Chorong reads as source, everywhere: these reads and the viewer's
/// [`crate::SourceBuffer`] share it, so a file that can be read can also be shown. Generated
/// bundles and data files beyond this are not useful to read in a code viewer and would cost
/// memory and parse time. 16 MiB, the viewer's long-standing limit (the stricter of the two
/// this replaced).
pub const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug)]
pub enum ReadError {
    Io(std::io::Error),
    /// A directory, FIFO, socket or device.
    NotAFile(PathBuf),
    TooLarge {
        path: PathBuf,
        len: u64,
        limit: u64,
    },
    /// The path resolves (through `..` or a symbolic link) outside the root.
    OutsideRoot {
        path: PathBuf,
        root: PathBuf,
    },
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
        match self {
            ReadError::Io(e) => write!(f, "{e}"),
            ReadError::NotAFile(p) => write!(f, "{} is not a regular file", p.display()),
            ReadError::TooLarge { path, len, limit } => write!(
                f,
                "{} is too large to read ({:.1} MB; the limit is {:.0} MB)",
                path.display(),
                mb(*len),
                mb(*limit)
            ),
            ReadError::OutsideRoot { path, root } => write!(
                f,
                "{} is outside {} (a symbolic link?), so Chorong does not read it",
                path.display(),
                root.display()
            ),
        }
    }
}

impl std::error::Error for ReadError {}

impl From<std::io::Error> for ReadError {
    fn from(e: std::io::Error) -> Self {
        ReadError::Io(e)
    }
}

impl From<ReadError> for std::io::Error {
    fn from(e: ReadError) -> Self {
        match e {
            ReadError::Io(e) => e,
            other => std::io::Error::other(other.to_string()),
        }
    }
}

/// Reads `path` (relative to `root`, or absolute) if it is a regular file inside `root` after
/// resolving symbolic links and `..`, no larger than [`MAX_SOURCE_BYTES`].
pub fn read_source_in(root: &Path, path: &Path) -> Result<Vec<u8>, ReadError> {
    let full = root.join(path);
    let real_root = root.canonicalize()?;
    let real = full.canonicalize()?;
    if !real.starts_with(&real_root) {
        return Err(ReadError::OutsideRoot {
            path: full,
            root: root.to_path_buf(),
        });
    }
    read_limited(&real, MAX_SOURCE_BYTES)
}

/// Reads a file the reader named themselves (a command-line argument): a regular file no
/// larger than [`MAX_SOURCE_BYTES`], wherever it is.
pub fn read_source(path: &Path) -> Result<Vec<u8>, ReadError> {
    read_limited(path, MAX_SOURCE_BYTES)
}

/// [`read_source`] decoded as UTF-8 (lossy), for JSON files the reader named.
pub fn read_source_text(path: &Path) -> Result<String, ReadError> {
    let bytes = read_source(path)?;
    Ok(String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into()))
}

/// [`read_source_in`] decoded as UTF-8 (lossy), for manifests such as `package.json`.
pub fn read_source_text_in(root: &Path, path: &Path) -> Result<String, ReadError> {
    let bytes = read_source_in(root, path)?;
    Ok(String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into()))
}

fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, ReadError> {
    // Checked before opening, because opening a FIFO blocks until a writer appears.
    if !std::fs::metadata(path)?.is_file() {
        return Err(ReadError::NotAFile(path.to_path_buf()));
    }
    let file = std::fs::File::open(path)?;
    // Checked again on the open file, in case the path was swapped in between.
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(ReadError::NotAFile(path.to_path_buf()));
    }
    let too_large = |len| ReadError::TooLarge {
        path: path.to_path_buf(),
        len,
        limit,
    };
    if meta.len() > limit {
        return Err(too_large(meta.len()));
    }
    // A file can grow after the check, and some files (`/proc`) report length 0.
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(too_large(bytes.len() as u64));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cho-fs-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_regular_files_inside_the_root() {
        let root = temp("ok");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
        assert_eq!(
            read_source_in(&root, Path::new("src/a.rs")).unwrap(),
            b"fn a() {}\n"
        );
        assert!(
            read_source_in(&root, &root.join("src/a.rs")).is_ok(),
            "absolute"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn refuses_escapes_directories_and_devices() {
        let root = temp("esc");
        let outside = temp("esc-outside");
        std::fs::write(outside.join("secret"), "s").unwrap();
        assert!(matches!(
            read_source_in(&root, &outside.join("secret")),
            Err(ReadError::OutsideRoot { .. })
        ));
        // `..` to a file that exists (the sibling folder `outside`).
        let up = Path::new("..")
            .join(outside.file_name().unwrap())
            .join("secret");
        assert!(matches!(
            read_source_in(&root, &up),
            Err(ReadError::OutsideRoot { .. })
        ));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.join("secret"), root.join("link")).unwrap();
            assert!(matches!(
                read_source_in(&root, Path::new("link")),
                Err(ReadError::OutsideRoot { .. })
            ));
            // A symlinked folder that leads out: the file under it is outside.
            std::os::unix::fs::symlink(&outside, root.join("dir")).unwrap();
            assert!(matches!(
                read_source_in(&root, Path::new("dir/secret")),
                Err(ReadError::OutsideRoot { .. })
            ));
            // A symlink that stays inside is fine.
            std::fs::write(root.join("real"), "r").unwrap();
            std::os::unix::fs::symlink(root.join("real"), root.join("inner")).unwrap();
            assert_eq!(read_source_in(&root, Path::new("inner")).unwrap(), b"r");
            assert!(matches!(
                read_source(Path::new("/dev/zero")),
                Err(ReadError::NotAFile(_))
            ));
        }
        assert!(matches!(read_source(&root), Err(ReadError::NotAFile(_))));
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn refuses_files_over_the_limit() {
        let root = temp("big");
        std::fs::write(root.join("big"), vec![b'x'; 2048]).unwrap();
        assert!(matches!(
            read_limited(&root.join("big"), 1024),
            Err(ReadError::TooLarge { len: 2048, .. })
        ));
        assert_eq!(read_limited(&root.join("big"), 4096).unwrap().len(), 2048);
        assert_eq!(read_limited(&root.join("big"), 2048).unwrap().len(), 2048);
        std::fs::remove_dir_all(&root).ok();
    }

    /// The public readers apply the real limit (a sparse file, so nothing large is written).
    #[test]
    fn the_public_readers_apply_the_limit() {
        let root = temp("limit");
        let f = std::fs::File::create(root.join("huge")).unwrap();
        f.set_len(MAX_SOURCE_BYTES + 1).unwrap();
        for result in [
            read_source_in(&root, Path::new("huge")),
            read_source(&root.join("huge")),
        ] {
            assert!(matches!(
                result,
                Err(ReadError::TooLarge { len, limit: MAX_SOURCE_BYTES, .. }) if len == MAX_SOURCE_BYTES + 1
            ));
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// Files whose length says 0 (`/proc`) or that grow after the check are cut at the limit
    /// while reading, not trusted by their metadata.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_file_longer_than_its_metadata_is_still_limited() {
        let path = Path::new("/proc/self/status");
        assert_eq!(std::fs::metadata(path).unwrap().len(), 0);
        assert!(matches!(
            read_limited(path, 16),
            Err(ReadError::TooLarge {
                len: 17,
                limit: 16,
                ..
            })
        ));
        assert!(read_limited(path, 1 << 20).unwrap().len() > 16);
    }

    /// A FIFO is refused without blocking (opening one for reading waits for a writer).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_fifo_is_refused_without_hanging() {
        let root = temp("fifo");
        let made = std::process::Command::new("mkfifo")
            .arg(root.join("pipe"))
            .status()
            .is_ok_and(|s| s.success());
        assert!(made, "mkfifo failed, so this test would check nothing");
        assert!(matches!(
            read_source_in(&root, Path::new("pipe")),
            Err(ReadError::NotAFile(_))
        ));
        std::fs::remove_dir_all(&root).ok();
    }
}
