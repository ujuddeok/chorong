//! Where the repository cache lives and how a repository maps to folders in it.
//!
//! ```text
//! <repos>/github/acme/api.git                  bare, blobless clone
//! <repos>/github/acme/api/state.json           worktree recency and fetch times
//! <repos>/github/acme/api/worktrees/pr-123     linked worktrees, detached
//! ```

use crate::{Error, Result};
use cho_core::RepoRef;
use std::path::{Path, PathBuf};

/// The environment variable that names the `repos` directory itself (tests, CI).
pub const REPOS_DIR_ENV: &str = "CHORONG_REPOS_DIR";

/// Chorong's repository cache. Every write Chorong makes for remote sources is inside `root`.
#[derive(Debug, Clone)]
pub struct Cache {
    root: PathBuf,
}

/// The folders of one repository in the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoPaths {
    /// The bare clone (`<name>.git`).
    pub bare: PathBuf,
    /// The folder beside it (`<name>/`) holding `state.json` and `worktrees/`.
    pub dir: PathBuf,
    pub worktrees: PathBuf,
    pub state: PathBuf,
    /// The advisory lock file (`.<name>.lock` beside the clone, so clearing the repository
    /// doesn't delete a lock someone holds).
    pub lock: PathBuf,
}

/// An advisory lock on one repository in the cache, held until dropped.
#[derive(Debug)]
pub struct RepoLock {
    _file: std::fs::File,
}

impl RepoPaths {
    pub fn worktree(&self, slug: &str) -> PathBuf {
        self.worktrees.join(slug)
    }
}

impl Cache {
    /// `$XDG_CACHE_HOME/chorong/repos` (macOS: `~/Library/Caches/chorong/repos`), or
    /// `CHORONG_REPOS_DIR` when set.
    pub fn default_location() -> Option<Self> {
        if let Some(dir) = std::env::var_os(REPOS_DIR_ENV).filter(|v| !v.is_empty()) {
            return Some(Self::at(dir));
        }
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from);
        let cache = if cfg!(target_os = "macos") {
            home?.join("Library/Caches")
        } else {
            match std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
                Some(x) => PathBuf::from(x),
                None => home?.join(".cache"),
            }
        };
        Some(Self::at(cache.join("chorong").join("repos")))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folders for `repo`. Refuses `local:` references and path components that could
    /// leave the cache (`..`, separators, leading dots).
    pub fn repo(&self, repo: &RepoRef) -> Result<RepoPaths> {
        let parts: Vec<&str> = match repo {
            RepoRef::GitHub { owner, name } => vec!["github", owner, name],
            RepoRef::Bitbucket { workspace, name } => vec!["bitbucket", workspace, name],
            RepoRef::BitbucketDc {
                host,
                project,
                name,
            } => vec!["bitbucket-dc", host, project, name],
            RepoRef::Local { .. } => {
                return Err(Error::unsupported(format!(
                    "`{repo}` is a local folder; only remote repositories have a cache"
                )));
            }
        };
        for p in &parts {
            check_component(p)?;
        }
        let (name, dirs) = parts.split_last().expect("non-empty");
        let parent = dirs.iter().fold(self.root.clone(), |acc, p| acc.join(p));
        let dir = parent.join(name);
        Ok(RepoPaths {
            lock: parent.join(format!(".{name}.lock")),
            bare: parent.join(format!("{name}.git")),
            worktrees: dir.join("worktrees"),
            state: dir.join("state.json"),
            dir,
        })
    }

    /// Takes the repository's lock: opening, fetching, placing and removing worktrees and
    /// clearing all hold it, so the viewer and the CLI never race on one clone. Waits up to
    /// `timeout`, then fails saying another Chorong is using the repository.
    pub fn lock(&self, paths: &RepoPaths, timeout: std::time::Duration) -> Result<RepoLock> {
        self.lock_with(paths, timeout, &crate::Cancel::new())
    }

    /// [`lock`](Self::lock), giving up early (`Cancelled`) when `cancel` is set.
    pub fn lock_with(
        &self,
        paths: &RepoPaths,
        timeout: std::time::Duration,
        cancel: &crate::Cancel,
    ) -> Result<RepoLock> {
        self.ensure_inside(&paths.lock)?;
        let dir = paths.lock.parent().expect("lock has a parent");
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::msg(format!("cannot create {}: {e}", dir.display())))?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&paths.lock)
            .map_err(|e| Error::msg(format!("cannot open {}: {e}", paths.lock.display())))?;
        let start = std::time::Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(RepoLock { _file: file }),
                Err(std::fs::TryLockError::WouldBlock) if cancel.is_cancelled() => {
                    return Err(Error::cancelled());
                }
                Err(std::fs::TryLockError::WouldBlock) if start.elapsed() < timeout => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(Error::new(
                        crate::ErrorKind::LockBusy,
                        format!(
                            "another Chorong has been using {} for {}s (opening or clearing it); \
                         try again when it finishes",
                            paths.bare.display(),
                            timeout.as_secs()
                        ),
                    ));
                }
                Err(std::fs::TryLockError::Error(e)) => {
                    return Err(Error::msg(format!(
                        "cannot lock {}: {e}",
                        paths.lock.display()
                    )));
                }
            }
        }
    }

    /// The repository and worktree folder name a path in the cache's `worktrees/` belongs
    /// to; `None` for anything else (a user's own folder).
    pub fn worktree_owner(&self, path: &Path) -> Option<(RepoRef, String)> {
        let rel = resolve(path)
            .strip_prefix(resolve(&self.root))
            .ok()?
            .to_path_buf();
        let parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let (kind, n) = match parts.first()?.as_str() {
            "github" | "bitbucket" => (parts[0].clone(), 2),
            "bitbucket-dc" => (parts[0].clone(), 3),
            _ => return None,
        };
        if parts.len() != n + 3 || parts[n + 1] != "worktrees" {
            return None;
        }
        let repo = RepoRef::parse(&format!("{kind}:{}", parts[1..=n].join("/"))).ok()?;
        Some((repo, parts[n + 2].clone()))
    }

    /// True when `path` is inside the cache. Both are compared as [`resolve`]d paths, so a
    /// symbolic link in the cache's own path (macOS `/var` is `/private/var`) does not make a
    /// path that does not exist yet look outside it.
    pub fn contains(&self, path: &Path) -> bool {
        resolve(path).starts_with(resolve(&self.root))
    }

    /// Fails unless `path` is inside the cache: the guard before any command that writes.
    pub fn ensure_inside(&self, path: &Path) -> Result<()> {
        if self.contains(path) {
            Ok(())
        } else {
            Err(Error::msg(format!(
                "refusing to write outside Chorong's cache: {}",
                path.display()
            )))
        }
    }
}

/// `p` with symbolic links resolved as far as it exists: the longest leading part that exists
/// is canonicalized (by the system, so a `..` after a link goes where a write would go) and the
/// rest, which does not exist and so holds no links, is added back with `.` dropped and `..`
/// applied to what is built so far.
fn resolve(p: &Path) -> PathBuf {
    let parts: Vec<_> = p.components().collect();
    for k in (1..=parts.len()).rev() {
        let head: PathBuf = parts[..k].iter().collect();
        if let Ok(real) = head.canonicalize() {
            return append(
                real,
                parts[k..].iter().map(|c| c.as_os_str().to_os_string()),
            );
        }
    }
    normalize(p)
}

/// `base` followed by `parts` (which do not exist on disk), `..` popping what was built.
fn append(mut base: PathBuf, parts: impl Iterator<Item = std::ffi::OsString>) -> PathBuf {
    for part in parts {
        match part.to_str() {
            Some("..") => {
                base.pop();
            }
            Some(".") => {}
            _ => base.push(part),
        }
    }
    base
}

/// `..` and `.` removed lexically (for paths that don't exist yet).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn check_component(p: &str) -> Result<()> {
    let bad = p.is_empty()
        || p.starts_with('.')
        || p.chars()
            .any(|c| matches!(c, '/' | '\\' | '\0') || c.is_control());
    if bad {
        Err(Error::msg(format!(
            "`{p}` cannot be used as a folder name in the cache"
        )))
    } else {
        Ok(())
    }
}

/// The worktree folder name for a branch: the name with characters outside
/// `[A-Za-z0-9._-]` replaced by `-`, plus a short hash of the real name when that changed it
/// (so `feat/x` and `feat-x` never share a folder) or when it would look like a pull request
/// or commit folder.
pub fn branch_slug(branch: &str) -> String {
    let clean: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let clean = clean.trim_start_matches('.').to_string();
    let reserved = clean.starts_with("pr-") || clean.starts_with("rev-") || clean.is_empty();
    if clean == branch && !reserved {
        clean
    } else {
        format!("{clean}~{:08x}", fnv1a(branch.as_bytes()) as u32)
    }
}

pub fn pr_slug(number: u64) -> String {
    format!("pr-{number}")
}

pub fn rev_slug(commit: &str) -> String {
    format!("rev-{}", &commit[..commit.len().min(12)])
}

pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let c = Cache::at("/c/repos");
        let p = c.repo(&RepoRef::parse("github:acme/api").unwrap()).unwrap();
        assert_eq!(p.bare, PathBuf::from("/c/repos/github/acme/api.git"));
        assert_eq!(
            p.worktree("pr-1"),
            PathBuf::from("/c/repos/github/acme/api/worktrees/pr-1")
        );
        let p = c
            .repo(&RepoRef::parse("bitbucket-dc:git.example.com/PRJ/api").unwrap())
            .unwrap();
        assert_eq!(
            p.bare,
            PathBuf::from("/c/repos/bitbucket-dc/git.example.com/PRJ/api.git")
        );
    }

    #[test]
    fn refuses_escapes() {
        let c = Cache::at("/c/repos");
        assert!(c.repo(&RepoRef::parse("github:../x").unwrap()).is_err());
        assert!(c.repo(&RepoRef::parse("github:a/.git").unwrap()).is_err());
        assert!(c.repo(&RepoRef::parse("local:/tmp/x").unwrap()).is_err());
        assert!(!c.contains(Path::new("/c/repos/../other")));
        assert!(c.contains(Path::new("/c/repos/github/a/b.git")));
    }

    #[test]
    fn worktree_owner() {
        let c = Cache::at("/c/repos");
        assert_eq!(
            c.worktree_owner(Path::new("/c/repos/github/acme/api/worktrees/pr-3")),
            Some((
                RepoRef::parse("github:acme/api").unwrap(),
                "pr-3".to_string()
            ))
        );
        assert_eq!(
            c.worktree_owner(Path::new("/c/repos/bitbucket-dc/h/P/r/worktrees/main"))
                .map(|(r, _)| r.to_string()),
            Some("bitbucket-dc:h/P/r".to_string())
        );
        assert!(
            c.worktree_owner(Path::new("/c/repos/github/acme/api.git"))
                .is_none()
        );
        assert!(
            c.worktree_owner(Path::new("/c/repos/github/acme/api/worktrees/pr-3/src"))
                .is_none()
        );
        assert!(c.worktree_owner(Path::new("/home/me/api")).is_none());
    }

    /// A cache whose own path goes through a symbolic link (macOS temp folders: `/var` is
    /// `/private/var`) still holds the paths under it that do not exist yet, and a link inside
    /// it that leads out still leads out.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_cache_holds_new_paths_and_links_out_stay_out() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir_all(real.join("repos")).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let via = dir.path().join("via");
        std::os::unix::fs::symlink(&real, &via).unwrap();
        std::os::unix::fs::symlink(&outside, real.join("repos/out")).unwrap();
        for root in [&real, &via] {
            let c = Cache::at(root);
            let new = root.join("repos/github/acme/api.git");
            assert!(c.contains(&new), "{} not yet there", new.display());
            assert!(c.ensure_inside(&new).is_ok());
            assert!(c.contains(&real.join("repos/github/acme/api.git")));
            assert!(c.contains(&via.join("repos/github/acme/api.git")));
            assert!(!c.contains(&root.join("repos/out/x")), "a link out");
            assert!(
                !c.contains(&root.join("repos/out/../escaped")),
                "a link out, then .."
            );
            assert!(!c.contains(&root.join("repos/../../outside/x")), "plain ..");
            assert!(c.contains(&root.join("repos/new/../still-in")));
        }
    }

    #[test]
    fn lock_is_exclusive_and_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::at(dir.path());
        let p = c.repo(&RepoRef::parse("github:acme/api").unwrap()).unwrap();
        let held = c.lock(&p, std::time::Duration::from_secs(1)).unwrap();
        let e = c
            .lock(&p, std::time::Duration::from_millis(200))
            .unwrap_err()
            .to_string();
        assert!(e.contains("another Chorong"), "{e}");
        drop(held);
        c.lock(&p, std::time::Duration::from_millis(200)).unwrap();
    }

    /// Waiting for a lock another Chorong holds ends at once when the wait is cancelled, long
    /// before the lock timeout, and does not take the lock.
    #[test]
    fn a_cancelled_wait_for_the_lock_returns_promptly() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::at(dir.path());
        let p = c.repo(&RepoRef::parse("github:acme/api").unwrap()).unwrap();
        let held = c.lock(&p, std::time::Duration::from_secs(1)).unwrap();
        let cancel = crate::Cancel::new();
        let c2 = cancel.clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(250));
            c2.cancel();
        });
        let start = std::time::Instant::now();
        let e = c
            .lock_with(&p, std::time::Duration::from_secs(120), &cancel)
            .unwrap_err();
        stopper.join().unwrap();
        assert_eq!(e.kind, crate::ErrorKind::Cancelled);
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
        drop(held);
        // The cancelled wait left nothing locked behind it.
        c.lock(&p, std::time::Duration::from_millis(200)).unwrap();
    }

    #[test]
    fn slugs() {
        assert_eq!(branch_slug("main"), "main");
        assert_ne!(branch_slug("feat/x"), branch_slug("feat-x"));
        assert!(branch_slug("feat/x").starts_with("feat-x~"));
        assert!(branch_slug("pr-3").starts_with("pr-3~"));
        let odd = branch_slug("../../x");
        assert!(!odd.contains('/') && !odd.starts_with('.'), "{odd}");
        assert_eq!(pr_slug(12), "pr-12");
        assert_eq!(rev_slug("0123456789abcdef"), "rev-0123456789ab");
    }
}
