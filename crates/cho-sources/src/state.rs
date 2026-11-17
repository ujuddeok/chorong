//! `state.json` beside each bare clone: which worktrees exist, in what order they were
//! opened (for eviction), the default branch, and when each ref was last fetched. It lives
//! in the cache, so clearing the cache clears it.

use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeState {
    /// Order of opening: higher is more recent. A counter, not a clock, so two opens in the
    /// same second still order.
    pub seq: u64,
    /// Seconds since the Unix epoch.
    pub opened: u64,
    pub head: String,
    /// What it was opened for, in words: `branch feat/x`, `pull request 12`, `commit 3f9c1e2`
    /// ([`OpenedAs::describe`]). Still written so older versions can read the file.
    pub what: String,
    /// What it was opened for. Files written before this field existed get it from `what`
    /// when they are loaded; `None` when that can't be read.
    #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
    pub opened_as: Option<OpenedAs>,
    /// The default branch's worktree, which is never evicted.
    #[serde(default)]
    pub default_branch: bool,
    /// A pull request's details, kept so it still has its header from recents or offline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<PrRecord>,
    /// The last prepare run in this worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepare: Option<crate::prepare::run::PrepareRecord>,
}

/// What a worktree was opened for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenedAs {
    Branch(String),
    PullRequest(u64),
    /// The full commit id (abbreviated in files written before [`WorktreeState::opened_as`]).
    Commit(String),
}

impl OpenedAs {
    /// `branch`, `pull-request` or `commit`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Branch(_) => "branch",
            Self::PullRequest(_) => "pull-request",
            Self::Commit(_) => "commit",
        }
    }

    /// In words: `branch feat/x`, `pull request 12`, `commit 3f9c1e2`.
    pub fn describe(&self) -> String {
        match self {
            Self::Branch(b) => format!("branch {b}"),
            Self::PullRequest(n) => format!("pull request {n}"),
            Self::Commit(c) => format!("commit {}", cho_git::short(c)),
        }
    }

    pub fn branch(&self) -> Option<&str> {
        match self {
            Self::Branch(b) => Some(b),
            _ => None,
        }
    }

    pub fn pull_request(&self) -> Option<u64> {
        match self {
            Self::PullRequest(n) => Some(*n),
            _ => None,
        }
    }

    /// From [`describe`](Self::describe)'s words, for state files written before
    /// [`WorktreeState::opened_as`] existed.
    fn from_description(what: &str) -> Option<Self> {
        if let Some(n) = what.strip_prefix("pull request ") {
            n.trim().parse().ok().map(Self::PullRequest)
        } else if let Some(b) = what.strip_prefix("branch ") {
            Some(Self::Branch(b.to_string()))
        } else {
            what.strip_prefix("commit ")
                .map(|c| Self::Commit(c.to_string()))
        }
    }
}

/// Pull-request details as read from the host, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrRecord {
    pub details: crate::hosts::PrDetails,
    /// Seconds since the Unix epoch.
    pub fetched_at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoState {
    #[serde(default)]
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    #[serde(default)]
    pub worktrees: BTreeMap<String, WorktreeState>,
    /// Local ref name → seconds since the Unix epoch of its last successful fetch.
    #[serde(default)]
    pub fetched: BTreeMap<String, u64>,
}

/// A folder name that stays inside the worktrees folder.
fn is_slug(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('.') && !s.contains(['/', '\\', '\0'])
}

impl RepoState {
    /// The saved state, or an empty one when there is none or it can't be read (it only
    /// orders eviction and reports fetch times; nothing is lost by starting again).
    #[allow(clippy::disallowed_methods)] // Chorong's own cache state file
    pub fn load(path: &Path) -> Self {
        let mut s: Self = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        s.normalise();
        s
    }

    /// Makes a hand-edited or stale file safe to use: slugs name folders directly inside the
    /// worktrees folder (an entry like `../other` would make eviction remove another
    /// repository's worktree), and the counter is not behind any worktree's, so the next
    /// opened worktree is the most recent.
    fn normalise(&mut self) {
        self.worktrees.retain(|slug, _| is_slug(slug));
        for w in self.worktrees.values_mut() {
            if w.opened_as.is_none() {
                w.opened_as = OpenedAs::from_description(&w.what);
            }
        }
        let top = self.worktrees.values().map(|w| w.seq).max().unwrap_or(0);
        self.seq = self.seq.max(top);
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self).expect("json") + "\n";
        crate::write_atomic(path, text.as_bytes())
    }

    /// The last prepare run in worktree `slug`.
    pub fn prepare_record(&self, slug: &str) -> Option<crate::prepare::run::PrepareRecord> {
        self.worktrees.get(slug).and_then(|w| w.prepare.clone())
    }

    pub fn touch(&mut self, slug: &str, mut w: WorktreeState) {
        self.seq = self.seq.saturating_add(1);
        w.seq = self.seq;
        self.worktrees.insert(slug.to_string(), w);
    }

    /// Worktrees to remove so that at most `cap` remain after `keep` is added: the least
    /// recently opened first, never the default branch's or `keep`.
    pub fn evictions(&self, keep: &str, cap: usize) -> Vec<String> {
        let mut others: Vec<(&String, &WorktreeState)> = self
            .worktrees
            .iter()
            .filter(|(s, _)| s.as_str() != keep)
            .collect();
        let total = others.len() + 1;
        if total <= cap {
            return vec![];
        }
        others.retain(|(_, w)| !w.default_branch);
        others.sort_by_key(|(s, w)| (w.seq, s.to_string()));
        others
            .into_iter()
            .take(total - cap)
            .map(|(s, _)| s.clone())
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // tests read files they wrote
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recent_but_never_default() {
        let mut s = RepoState::default();
        let w = |default_branch| WorktreeState {
            default_branch,
            prepare: None,
            pull_request: None,
            ..Default::default()
        };
        s.touch("main", w(true));
        s.touch("a", w(false));
        s.touch("b", w(false));
        s.touch("c", w(false));
        assert_eq!(s.evictions("d", 4), vec!["a".to_string()]);
        assert_eq!(s.evictions("d", 3), vec!["a".to_string(), "b".to_string()]);
        assert!(s.evictions("a", 4).is_empty());
        // Re-opening `a` makes `b` the oldest.
        s.touch("a", w(false));
        assert_eq!(s.evictions("d", 4), vec!["b".to_string()]);
        // A cap of 1 can't remove the default branch's worktree.
        assert_eq!(s.evictions("d", 1).len(), 3);
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x").join("state.json");
        let mut s = RepoState {
            default_branch: Some("main".into()),
            ..Default::default()
        };
        s.fetched.insert("refs/heads/main".into(), 5);
        s.save(&p).unwrap();
        assert_eq!(RepoState::load(&p), s);
        std::fs::write(&p, "garbage").unwrap();
        assert_eq!(RepoState::load(&p), RepoState::default());
    }

    /// Files written before `as` existed name what was opened only in words.
    #[test]
    fn old_files_get_what_was_opened_from_the_words() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        let w = |what: &str| format!(r#"{{"seq":1,"opened":0,"head":"","what":"{what}"}}"#);
        std::fs::write(
            &p,
            format!(
                r#"{{"worktrees":{{"a":{},"b":{},"c":{},"d":{}}}}}"#,
                w("branch feat/x"),
                w("pull request 12"),
                w("commit 3f9c1e2"),
                w("tag v1")
            ),
        )
        .unwrap();
        let s = RepoState::load(&p);
        let got: Vec<_> = s.worktrees.values().map(|w| w.opened_as.clone()).collect();
        assert_eq!(
            got,
            [
                Some(OpenedAs::Branch("feat/x".into())),
                Some(OpenedAs::PullRequest(12)),
                Some(OpenedAs::Commit("3f9c1e2".into())),
                None
            ]
        );
        // Saved again, the typed value is kept beside the words.
        s.save(&p).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains(r#""pull-request": 12"#), "{text}");
        assert_eq!(RepoState::load(&p), s);
        for a in got.iter().flatten() {
            assert_eq!(OpenedAs::from_description(&a.describe()).as_ref(), Some(a));
        }
    }

    /// Found by the `persist_state` fuzz target: a hand-edited `state.json` could name a
    /// worktree `../other`, and a counter behind (or at the end of) the worktrees' own made
    /// the next opened worktree look the oldest, or overflowed.
    #[test]
    fn load_repairs_slugs_and_the_counter() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        let w = |seq: u64| format!("{{\"seq\":{seq},\"opened\":0,\"head\":\"\",\"what\":\"\"}}");
        std::fs::write(
            &p,
            format!(
                "{{\"seq\":1,\"worktrees\":{{\"../other\":{},\".git\":{},\"a/b\":{},\"ok\":{}}}}}",
                w(2),
                w(2),
                w(2),
                w(7)
            ),
        )
        .unwrap();
        let mut s = RepoState::load(&p);
        assert_eq!(s.worktrees.keys().collect::<Vec<_>>(), ["ok"]);
        assert_eq!(s.seq, 7);
        s.touch("new", WorktreeState::default());
        assert_eq!(s.worktrees["new"].seq, 8);
        // The end of the counter doesn't overflow.
        s.seq = u64::MAX;
        s.touch("newer", WorktreeState::default());
        assert_eq!(s.seq, u64::MAX);
    }
}
