//! Whether the checkout is at the investigation's `rev`, and reading files at that commit.
//!
//! Git runs only through [`cho_core::git::command`]. Every argument is either fixed or a
//! validated value: `rev` is hex digits (checked by [`crate::Investigation::validate`]) and
//! anchor paths are repository-relative with no `.` or `..`, so nothing can be read as an
//! option.

use crate::check::SourceReader;
use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// How the checkout relates to the investigation's pinned commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Drift {
    /// HEAD is `rev`. (Uncommitted changes are not looked at.)
    AtRev { head: String },
    /// HEAD is another commit, or unborn (`head` is `None`). `rev_present` says whether
    /// the pinned commit exists locally, so it can be read with [`CommitReader`].
    Differs {
        head: Option<String>,
        rev_present: bool,
    },
    /// `root` is not inside a git working tree, or git could not run.
    NotGitRepo { reason: String },
}

impl Drift {
    pub fn is_at_rev(&self) -> bool {
        matches!(self, Drift::AtRev { .. })
    }
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = cho_core::git::command(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// The full id of `rev` if it names a commit in the repository at `root`.
pub fn resolve_commit(root: &Path, rev: &str) -> Option<String> {
    if !rev.bytes().all(|b| b.is_ascii_hexdigit()) || rev.is_empty() {
        return None;
    }
    commit_of(root, rev).ok().flatten()
}

/// The full id of the commit `rev` names (`rev^{commit}`, after `--end-of-options`), or `None`
/// when it names none. `Err` only when git could not run. The caller validates `rev` first.
pub(crate) fn commit_of(root: &Path, rev: &str) -> Result<Option<String>, String> {
    let out = cho_core::git::command(root)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{rev}^{{commit}}"),
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(
        (out.status.success() && !id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit()))
            .then_some(id),
    )
}

/// Compares the checkout's HEAD with `rev`.
pub fn drift(root: &Path, rev: &str) -> Drift {
    match git(root, &["rev-parse", "--is-inside-work-tree"]) {
        Ok(s) if s == "true" => {}
        Ok(_) => {
            return Drift::NotGitRepo {
                reason: "not inside a git working tree".into(),
            };
        }
        Err(e) => return Drift::NotGitRepo { reason: e },
    }
    let head = git(root, &["rev-parse", "--verify", "--quiet", "HEAD"])
        .ok()
        .filter(|s| !s.is_empty());
    let pinned = resolve_commit(root, rev);
    match (&head, &pinned) {
        (Some(h), Some(p)) if h == p => Drift::AtRev { head: h.clone() },
        _ => Drift::Differs {
            head,
            rev_present: pinned.is_some(),
        },
    }
}

/// Reads anchored files from a commit with `git cat-file`, never from the working tree.
/// Each file is checked to be a blob no larger than [`cho_core::fs::MAX_SOURCE_BYTES`]
/// before it is read, and read once per reader.
pub struct CommitReader {
    root: PathBuf,
    commit: String,
    cache: RefCell<HashMap<String, Result<Vec<u8>, String>>>,
}

impl CommitReader {
    /// A reader for `rev` in the repository at `root`, or why it can't be read.
    pub fn new(root: &Path, rev: &str) -> Result<Self, String> {
        let commit = resolve_commit(root, rev).ok_or_else(|| {
            format!(
                "commit {rev} is not in the repository at {}",
                root.display()
            )
        })?;
        Ok(Self {
            root: root.to_path_buf(),
            commit,
            cache: RefCell::default(),
        })
    }

    /// The full commit id being read.
    pub fn commit(&self) -> &str {
        &self.commit
    }

    fn read_uncached(&self, path: &str) -> Result<Vec<u8>, String> {
        let short = &self.commit[..self.commit.len().min(12)];
        self.read_blob(path)?
            .ok_or_else(|| format!("`{path}` does not exist at {short}"))
    }

    /// The bytes of `path` at the commit; `Ok(None)` when the commit has no such path.
    pub fn read_blob(&self, path: &str) -> Result<Option<Vec<u8>>, String> {
        if path.is_empty()
            || path.starts_with('/')
            || path
                .split('/')
                .any(|c| c.is_empty() || c == "." || c == "..")
            || path.chars().any(char::is_control)
        {
            return Err(format!("`{path}` is not a repository-relative path"));
        }
        let short = &self.commit[..self.commit.len().min(12)];
        let spec = format!("{}:{path}", self.commit);
        // `--batch-check` reads the object name from stdin, so the path is never an argument.
        let mut child = cho_core::git::command(&self.root)
            .args(["cat-file", "--batch-check"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot run git: {e}"))?;
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(format!("{spec}\n").as_bytes())
            .map_err(|e| format!("cannot run git: {e}"))?;
        let out = child
            .wait_with_output()
            .map_err(|e| format!("cannot run git: {e}"))?;
        let line = String::from_utf8_lossy(&out.stdout);
        let mut parts = line.split_whitespace();
        let (Some(oid), Some(kind), Some(size)) = (parts.next(), parts.next(), parts.next()) else {
            return Ok(None);
        };
        if kind != "blob" {
            return Err(format!("`{path}` is a {kind} at {short}, not a file"));
        }
        let size: u64 = size
            .parse()
            .map_err(|_| "unexpected git output".to_string())?;
        if size > cho_core::fs::MAX_SOURCE_BYTES {
            return Err(format!(
                "`{path}` is too large to read at {short} ({size} bytes)"
            ));
        }
        if !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("unexpected git output".into());
        }
        let out = cho_core::git::command(&self.root)
            .args(["cat-file", "blob", oid])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("cannot run git: {e}"))?;
        if !out.status.success() {
            return Err(format!("cannot read `{path}` at {short}"));
        }
        Ok(Some(out.stdout))
    }
}

impl SourceReader for CommitReader {
    fn root(&self) -> Option<&Path> {
        Some(&self.root)
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        if let Some(hit) = self.cache.borrow().get(path) {
            return hit.clone();
        }
        let result = self.read_uncached(path);
        self.cache
            .borrow_mut()
            .insert(path.to_string(), result.clone());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Investigation;
    use crate::check::{NodeStatus, check, check_with_reader};
    use cho_core::test_repo;
    use std::fs;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cho-drift-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn inv(rev: &str) -> Investigation {
        Investigation::parse(
            &serde_json::json!({
                "chorong": 1, "title": "t", "kind": "tour", "repo": "local:/x", "rev": rev,
                "nodes": [
                    { "id": "a", "anchor": { "path": "src/a.rs", "symbol": "alpha" } },
                    { "id": "b", "anchor": { "path": "src/a.rs", "symbol": "beta" } }
                ],
                "steps": ["a", "b"]
            })
            .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn drift_states_and_reading_at_the_pinned_commit() {
        let dir = temp("states");
        test_repo::init(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();
        let first = test_repo::commit_all(&dir, "one");
        fs::write(dir.join("src/a.rs"), "fn alpha() {}\nfn gamma() {}\n").unwrap();
        let second = test_repo::commit_all(&dir, "two");

        assert_eq!(
            drift(&dir, &second),
            Drift::AtRev {
                head: second.clone()
            }
        );
        assert_eq!(
            drift(&dir, &second[..7]),
            Drift::AtRev {
                head: second.clone()
            }
        );
        assert_eq!(
            drift(&dir, &first),
            Drift::Differs {
                head: Some(second.clone()),
                rev_present: true
            }
        );
        assert_eq!(
            drift(&dir, "deadbeefdeadbeef"),
            Drift::Differs {
                head: Some(second.clone()),
                rev_present: false
            }
        );

        // In the working tree `beta` is gone; at the pinned commit it resolves.
        let pinned = inv(&first);
        let worktree = check(&pinned, &dir);
        assert_eq!(worktree.nodes[1].status, NodeStatus::Broken);
        let reader = CommitReader::new(&dir, &first[..9]).unwrap();
        assert_eq!(reader.commit(), first);
        let at_rev = check_with_reader(&pinned, &reader, None);
        assert_eq!(at_rev.nodes[0].status, NodeStatus::Resolved);
        assert_eq!(at_rev.nodes[1].status, NodeStatus::Resolved);
        assert_eq!(at_rev.nodes[1].line, Some(2));

        assert!(
            reader
                .read("src/missing.rs")
                .unwrap_err()
                .contains("does not exist")
        );
        assert!(reader.read("src").unwrap_err().contains("tree"));
        assert!(reader.read("../x").is_err());
        assert!(CommitReader::new(&dir, "deadbeefdeadbeef").is_err());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn not_a_repository() {
        let dir = temp("norepo");
        // A temp dir may sit inside some outer repository; point at one that can't.
        let missing = dir.join("nope");
        assert!(matches!(
            drift(&missing, "abcdef1"),
            Drift::NotGitRepo { .. }
        ));
        fs::remove_dir_all(dir).ok();
    }
}
