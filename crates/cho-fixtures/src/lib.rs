//! Fixtures shared by the tests of every crate (and by the verification scripts, through
//! the `seed-fixture` binary). Not shipped.
//!
//! [`TsWorkspace`] is a small TypeScript monorepo in one of the layouts described in
//! `docs/typescript-workspaces.md`, under `fixtures/ts-workspaces/`, with the SCIP index the
//! pinned scip-typescript produced for it committed beside it. Tests use it three ways:
//!
//! - [`TsWorkspace::semantic`]: the index loaded in place, for queries and the graph;
//! - [`TsWorkspace::checkout`]: a private copy, for tests that write or need their own root;
//! - [`TsWorkspace::seed_cache`]: a copy plus a Chorong index cache that already holds its
//!   index, so the real `cho` CLI or viewer runs against it with no indexer installed.

use cho_semantic::cache::{GitState, IndexCache, META_FILE, Meta, now_unix};
use cho_semantic::{IndexStatus, ScipIndex, Semantic};
use std::io;
use std::path::{Path, PathBuf};

/// A committed TypeScript workspace fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TsWorkspace {
    /// Packages export built declarations: `types`/`exports` to `dist/*.d.ts`, per file
    /// (with and without declaration maps, tsconfig `outDir`/`rootDir`, a `source` field)
    /// and bundled. Uses from other packages name the `.d.ts`; the dist→source join links them.
    Dist,
    /// Packages export source through a `development` condition, `dist/` is JavaScript only,
    /// and `tsc` emits `.types/*.d.ts` (with maps) through project references; one package
    /// exports source subpaths directly. Uses from other packages name the source.
    SourceCondition,
}

impl TsWorkspace {
    pub const ALL: [TsWorkspace; 2] = [Self::Dist, Self::SourceCondition];

    /// The folder name under `fixtures/ts-workspaces/`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Dist => "dist",
            Self::SourceCondition => "source-condition",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|w| w.name() == name)
    }

    /// The committed fixture. Read it; don't write into it (use [`TsWorkspace::checkout`]).
    pub fn dir(self) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/ts-workspaces")
            .join(self.name())
    }

    /// Its committed SCIP index (paths relative to the fixture root).
    pub fn index_path(self) -> PathBuf {
        self.dir().join("index.scip")
    }

    /// The index loaded against the committed fixture, with the dist→source join computed,
    /// as `Semantic::open` would load it.
    pub fn semantic(self) -> Semantic {
        let index = ScipIndex::load(&self.index_path(), "").expect("the fixture index loads");
        Semantic::new(index, self.dir(), IndexStatus::Current).with_dist_links()
    }

    /// Copies the fixture to `dest` (created; must not exist) and returns it canonicalised.
    pub fn checkout(self, dest: &Path) -> io::Result<PathBuf> {
        copy_dir(&self.dir(), dest)?;
        dest.canonicalize()
    }

    /// Copies the fixture to `dest` and records its committed index in the index cache at
    /// `cache` for that copy, as `cho index` would have. Point the CLI or viewer at it with
    /// `CHORONG_CACHE_DIR=<cache>` and `--root <returned path>`. The copy is made a Git
    /// repository with one commit, and the index is recorded at that commit, so it reads
    /// `current` until a test changes a file (then `stale`, as for a real checkout).
    pub fn seed_cache(self, dest: &Path, cache: &Path) -> io::Result<PathBuf> {
        let root = self.checkout(dest)?;
        commit_all(&root)?;
        let git = GitState::of(&root);
        let dir = IndexCache::at(cache).repo_dir(&root).join("typescript");
        std::fs::create_dir_all(&dir)?;
        std::fs::copy(self.index_path(), dir.join("index.scip"))?;
        let meta = Meta {
            indexer: "scip-typescript".into(),
            indexer_version: Some("0.6.0".into()),
            language: cho_semantic::Language::TypeScript,
            project_root: String::new(),
            git_head: git.head,
            git_status_hash: git.status_hash,
            created: now_unix(),
            result: cho_semantic::BuildResult::Built,
            duration_ms: 0,
            stderr_tail: String::new(),
            prepare: None,
        };
        let text = serde_json::to_string_pretty(&meta).expect("meta serialises");
        std::fs::write(dir.join(META_FILE), text)?;
        Ok(root)
    }
}

/// `git init` and one commit of everything, with a fixed author and date so the commit is
/// the same on every machine.
fn commit_all(root: &Path) -> io::Result<()> {
    let run = |args: &[&str]| -> io::Result<()> {
        let out = cho_core::git::own_repo_command(root)
            .args(args)
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_AUTHOR_DATE", "2026-11-02T00:00:00Z")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_DATE", "2026-11-02T00:00:00Z")
            .output()?;
        if out.status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )))
        }
    };
    run(&["init", "-q", "-b", "main"])?;
    run(&["add", "-A"])?;
    run(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-q",
        "-m",
        "fixture",
    ])
}

/// Copies a directory tree (regular files and folders; the fixtures have no links).
#[allow(clippy::disallowed_methods)] // Chorong's own committed fixtures, copied for a test
fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fixture_loads_and_has_documents() {
        for w in TsWorkspace::ALL {
            assert_eq!(TsWorkspace::from_name(w.name()), Some(w));
            let sem = w.semantic();
            assert!(!sem.index().documents().is_empty(), "{}", w.name());
            assert!(w.dir().join("README.md").is_file(), "{}", w.name());
        }
    }

    #[test]
    fn a_seeded_cache_opens_like_an_indexed_checkout() {
        let base = std::env::temp_dir().join(format!("cho-fixtures-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        let root = TsWorkspace::Dist
            .seed_cache(&base.join("ws"), &base.join("cache"))
            .unwrap();
        let sem = Semantic::open(&IndexCache::at(base.join("cache")), &root)
            .unwrap()
            .expect("an index");
        assert_eq!(sem.status(), IndexStatus::Current);
        assert!(root.join(".git").is_dir());
        assert!(!sem.dist_links().is_empty());
        std::fs::remove_dir_all(&base).ok();
    }
}
