//! Which repositories the reader has allowed Chorong to index (each build still starts only when
//! the reader clicks: design/manual-start.md).
//!
//! Indexers run a project's own code (Rust build scripts and procedural macros, TypeScript
//! tooling), so the first build in a repository asks. The answer is kept by canonical path in
//! Chorong's own settings file (`$XDG_CONFIG_HOME/chorong/trusted.json`; macOS:
//! `~/Library/Application Support/chorong/trusted.json`; `CHORONG_CONFIG_DIR` replaces the `cho`
//! directory for tests and screenshots), never in the repository.
//!
//! A worktree in Chorong's own repository cache (a remote project opened with `cho open`) is
//! never decided here: its trust is the project's prepare/index approval
//! (`cho_sources::index_permission`), and `trusted.json` is neither read nor
//! written for it. A local folder's trust never approves a cache worktree, or the reverse.
//!
//! Each entry records the repository's remote URL too. If a trusted path later has a different
//! remote (a different project unpacked or cloned to the same place), it is not trusted until
//! the reader says so again.

use cho_sources::Permission;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Default, Debug, PartialEq, Eq)]
struct File {
    /// Paths trusted by older versions, before the remote was recorded. Read as trusted and
    /// moved into `folders` with the current remote the first time they are checked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    trusted: Vec<String>,
    #[serde(default)]
    folders: Vec<Entry>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
struct Entry {
    path: String,
    /// The repository's remote URL when it was trusted (`None`: no remote, or not a
    /// repository). A different remote now means different code at the same path.
    #[serde(default)]
    remote: Option<String>,
}

/// What the store says about a folder.
#[derive(Debug, PartialEq, Eq)]
pub enum Trust {
    Trusted,
    Untrusted,
    /// Trusted, but its remote has changed since: something else is at this path now.
    Changed {
        was: Option<String>,
        now: Option<String>,
    },
    /// One of Chorong's cache worktrees whose project approval doesn't cover it; the reason
    /// says how to approve (`cho prepare approve REPO`).
    NeedsApproval(String),
}

/// An entry from an older version to record with its remote: what a read-only check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upgrade {
    path: String,
    remote: Option<String>,
}

/// Cloned to run a check off the UI thread (it is a path and two function pointers).
#[derive(Clone)]
pub struct TrustStore {
    path: Option<PathBuf>,
    /// The folder's identity: its remote URL. Replaceable for tests.
    remote: fn(&Path) -> Option<String>,
    /// For Chorong's cache worktrees, the project approval ([`Permission::Local`] for other
    /// folders). Replaceable for tests.
    permission: fn(&Path) -> Permission,
}

impl TrustStore {
    pub fn default_location() -> Self {
        Self {
            path: crate::prefs::file("trusted.json"),
            remote: cho_git::identity_remote,
            permission: cho_sources::index_permission,
        }
    }

    #[cfg(test)]
    pub fn at(path: impl Into<PathBuf>, remote: fn(&Path) -> Option<String>) -> Self {
        Self {
            path: Some(path.into()),
            remote,
            permission: |_| Permission::Local,
        }
    }

    fn read(&self) -> File {
        self.path
            .as_deref()
            .map(crate::prefs::load)
            .unwrap_or_default()
    }

    /// Is the repository at `canonical_root` trusted, with the same remote as when it was?
    /// (Tests; the viewer checks off the UI thread with `check_read_only`.)
    #[cfg(test)]
    pub fn is_trusted(&self, canonical_root: &Path) -> bool {
        self.check(canonical_root) == Trust::Trusted
    }

    /// Whether `canonical_root` is trusted, and whether its remote still matches. An entry
    /// from an older version (no remote recorded) is trusted and upgraded with the current
    /// remote. (Tests; the viewer checks off the UI thread with `check_read_only` and writes
    /// the upgrade on the UI thread.)
    #[cfg(test)]
    pub fn check(&self, canonical_root: &Path) -> Trust {
        let (trust, upgrade) = self.check_read_only(canonical_root);
        if let Some(u) = upgrade {
            // Best effort: if it can't be saved, the next check upgrades it instead.
            self.upgrade(u).ok();
        }
        trust
    }

    /// [`check`](Self::check) without writing, for a check off the UI thread: an entry from
    /// an older version reads as trusted, and the [`Upgrade`] it needs is returned for the
    /// caller to apply with [`upgrade`](Self::upgrade) on the thread that writes the file.
    pub fn check_read_only(&self, canonical_root: &Path) -> (Trust, Option<Upgrade>) {
        match (self.permission)(canonical_root) {
            Permission::Local => {}
            Permission::CacheApproved { .. } => return (Trust::Trusted, None),
            Permission::CacheNeedsApproval { reason, .. } => {
                return (Trust::NeedsApproval(reason), None);
            }
        }
        let key = canonical_root.to_string_lossy();
        let file = self.read();
        let now = (self.remote)(canonical_root);
        if let Some(e) = file.folders.iter().find(|e| e.path == key) {
            let trust = if e.remote == now {
                Trust::Trusted
            } else {
                Trust::Changed {
                    was: e.remote.clone(),
                    now,
                }
            };
            return (trust, None);
        }
        if file.trusted.iter().any(|t| *t == key) {
            let upgrade = Upgrade {
                path: key.into_owned(),
                remote: now,
            };
            return (Trust::Trusted, Some(upgrade));
        }
        (Trust::Untrusted, None)
    }

    /// Records an older entry's remote (from [`check_read_only`](Self::check_read_only)),
    /// only while the entry is still the old kind: an answer the reader gave since wins.
    pub fn upgrade(&self, upgrade: Upgrade) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Err(std::io::Error::other("no settings directory"));
        };
        let mut file = self.read();
        let legacy = file.trusted.contains(&upgrade.path);
        if !legacy || file.folders.iter().any(|e| e.path == upgrade.path) {
            return Ok(());
        }
        file.trusted.retain(|t| *t != upgrade.path);
        file.folders.push(Entry {
            path: upgrade.path,
            remote: upgrade.remote,
        });
        crate::prefs::save(path, &file)
    }

    /// Remembers `canonical_root` with its current remote.
    pub fn trust(&self, canonical_root: &Path) -> std::io::Result<()> {
        if (self.permission)(canonical_root) != Permission::Local {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "this is a remote project in Chorong's cache; approve it with `cho prepare approve`",
            ));
        }
        let Some(path) = &self.path else {
            return Err(std::io::Error::other("no settings directory"));
        };
        let mut file = self.read();
        let key = canonical_root.to_string_lossy().into_owned();
        file.trusted.retain(|t| *t != key);
        file.folders.retain(|e| e.path != key);
        file.folders.push(Entry {
            path: key,
            remote: (self.remote)(canonical_root),
        });
        crate::prefs::save(path, &file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_only_check_of_an_older_entry_writes_nothing_until_upgraded() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("trusted.json");
        std::fs::write(&file, r#"{ "trusted": ["/work/repo"] }"#).unwrap();
        let store = TrustStore::at(&file, |_| Some("git@host:a.git".into()));
        let repo = Path::new("/work/repo");
        let (trust, upgrade) = store.check_read_only(repo);
        assert_eq!(trust, Trust::Trusted);
        assert_eq!(store.read().trusted, ["/work/repo"], "nothing written yet");
        store
            .upgrade(upgrade.expect("an older entry needs its remote"))
            .unwrap();
        let read = store.read();
        assert!(read.trusted.is_empty());
        assert_eq!(read.folders[0].remote.as_deref(), Some("git@host:a.git"));
        assert_eq!(store.check_read_only(repo), (Trust::Trusted, None));
    }

    #[test]
    fn an_upgrade_does_not_undo_an_answer_given_since() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("trusted.json");
        std::fs::write(&file, r#"{ "trusted": ["/work/repo"] }"#).unwrap();
        let stale = TrustStore::at(&file, |_| Some("old".into()));
        let (_, upgrade) = stale.check_read_only(Path::new("/work/repo"));
        // The reader trusts the folder again (with its remote now) before the upgrade lands.
        TrustStore::at(&file, |_| Some("new".into()))
            .trust(Path::new("/work/repo"))
            .unwrap();
        stale.upgrade(upgrade.unwrap()).unwrap();
        let read = stale.read();
        assert_eq!(read.folders.len(), 1);
        assert_eq!(read.folders[0].remote.as_deref(), Some("new"));
    }

    #[test]
    fn trust_is_remembered_by_path_and_survives_reopening() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let file = dir.join("nested/trusted.json");
        let store = TrustStore::at(&file, |_| None);
        let repo = Path::new("/work/repo");
        assert!(!store.is_trusted(repo));
        store.trust(repo).unwrap();
        store.trust(repo).unwrap();
        assert!(store.is_trusted(repo));
        assert!(!store.is_trusted(Path::new("/work/other")));
        assert!(
            TrustStore::at(&file, |_| None).is_trusted(repo),
            "read back from disk"
        );
        assert_eq!(store.read().folders.len(), 1, "no duplicates");
    }

    #[test]
    fn a_missing_or_corrupt_file_trusts_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("trusted.json"), "not json").unwrap();
        assert!(!TrustStore::at(dir.join("trusted.json"), |_| None).is_trusted(Path::new("/x")));
        assert!(!TrustStore::at(dir.join("absent.json"), |_| None).is_trusted(Path::new("/x")));
    }

    #[test]
    fn a_different_remote_at_a_trusted_path_is_not_trusted() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let file = dir.join("trusted.json");
        let repo = Path::new("/work/repo");
        TrustStore::at(&file, |_| Some("git@github.com:acme/api.git".into()))
            .trust(repo)
            .unwrap();
        let same = TrustStore::at(&file, |_| Some("git@github.com:acme/api.git".into()));
        assert_eq!(same.check(repo), Trust::Trusted);
        let other = TrustStore::at(&file, |_| Some("git@github.com:evil/api.git".into()));
        assert_eq!(
            other.check(repo),
            Trust::Changed {
                was: Some("git@github.com:acme/api.git".into()),
                now: Some("git@github.com:evil/api.git".into()),
            }
        );
        assert!(!other.is_trusted(repo));
        // Trusting again records the new remote.
        other.trust(repo).unwrap();
        assert!(other.is_trusted(repo));
        assert!(!same.is_trusted(repo));
    }

    #[test]
    fn entries_from_older_versions_are_trusted_and_upgraded() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let file = dir.join("trusted.json");
        std::fs::write(&file, r#"{ "trusted": ["/work/repo"] }"#).unwrap();
        let store = TrustStore::at(&file, |_| Some("https://example.com/a.git".into()));
        assert_eq!(store.check(Path::new("/work/repo")), Trust::Trusted);
        let upgraded = store.read();
        assert!(upgraded.trusted.is_empty());
        assert_eq!(
            upgraded.folders,
            [Entry {
                path: "/work/repo".into(),
                remote: Some("https://example.com/a.git".into()),
            }]
        );
        let moved = TrustStore::at(&file, |_| Some("https://example.com/b.git".into()));
        assert!(!moved.is_trusted(Path::new("/work/repo")));
    }

    /// A cache worktree's trust is its project approval, whatever `trusted.json` says, and
    /// accepting the trust prompt can't write it.
    #[test]
    fn cache_worktrees_follow_the_project_approval() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let file = dir.join("trusted.json");
        let plain = TrustStore::at(&file, |_| None);
        let cache = Path::new("/cache/repos/github/acme/api/worktrees/pr-1");
        plain.trust(cache).unwrap();
        let approved = TrustStore {
            permission: |_| Permission::CacheApproved {
                repo: "github:acme/api".into(),
            },
            ..TrustStore::at(&file, |_| None)
        };
        let denied = TrustStore {
            permission: |_| Permission::CacheNeedsApproval {
                repo: "github:acme/api".into(),
                reason: "approve it".into(),
                recipe_changed: false,
                changed: vec![],
            },
            ..TrustStore::at(&file, |_| None)
        };
        // trusted.json lists the path, but the approval decides.
        assert_eq!(
            denied.check(cache),
            Trust::NeedsApproval("approve it".into())
        );
        assert_eq!(approved.check(cache), Trust::Trusted);
        assert!(!denied.is_trusted(cache));
        // The other direction: a local folder trusted in trusted.json is decided by it alone.
        let local = Path::new("/home/me/proj");
        plain.trust(local).unwrap();
        assert!(plain.is_trusted(local) && plain.check(local) == Trust::Trusted);
        let e = denied.trust(cache).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
    }
}
