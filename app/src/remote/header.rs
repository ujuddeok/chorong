//! The remote header band: what a window on one of Chorong's cache worktrees says about what it
//! shows, and the decisions around it. Plain data and pure functions (no I/O, no UI); the
//! adapter fills `Header` from `Opened` or from the cache, and `view::remote` draws it.

use cho_core::RepoRef;

/// "This is Chorong's copy" tooltip of Open on GitHub / Bitbucket.
pub const COPY_NOTE: &str = "This is Chorong's copy. To edit, open your own clone in Chorong.";

/// A relative time as the command-line program words it ("just now", "12 min ago").
pub use cho_core::words::ago;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Merged,
    Declined,
}

impl PrState {
    pub fn label(self) -> &'static str {
        match self {
            PrState::Open => "open",
            PrState::Merged => "merged",
            PrState::Declined => "declined",
        }
    }
}

/// What the host said about a pull request (the backend saves it with the worktree).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrInfo {
    pub title: String,
    pub author: String,
    pub state: PrState,
    /// When the host was asked (Unix seconds).
    pub fetched_at: Option<u64>,
    /// The details were saved earlier (a window reopened from recents, or the host could not
    /// be reached on this open) rather than fetched by this open: the band says how old.
    pub saved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    PullRequest {
        number: u64,
        info: Option<PrInfo>,
    },
    Branch(String),
    Commit(String),
    /// Something the cache recorded that this version does not know: shown as it is.
    Other(String),
}

/// Everything the band shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub repo: RepoRef,
    pub kind: Kind,
    /// After, as a full commit id.
    pub head: String,
    /// The branch the head is compared with.
    pub target: Option<String>,
    /// Commits on the target the head lacks (`None`: not known yet).
    pub behind: Option<usize>,
    /// When the target was last fetched (Unix seconds).
    pub fetched_at: Option<u64>,
    /// The last fetch failed and the cached refs were used.
    pub offline: bool,
}

/// The band's words, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    /// `#123`, "pull request 12", the project, or "commit 3f9c1e2".
    pub lead: String,
    /// The pull request title, or the second part for a branch ("branch feat/login").
    pub title: Option<String>,
    pub author: Option<String>,
    pub state: Option<PrState>,
    /// "details from 2 h ago", whenever the details did not come from this open (no
    /// threshold, however recent).
    pub stale: Option<String>,
    /// "3 behind main" (only when behind).
    pub behind: Option<String>,
    /// "main fetched 12 min ago", or "offline · last fetch 2 h ago".
    pub freshness: Option<String>,
}

/// The tooltip on `behind`.
pub const BEHIND_TIP: &str = "must be rebased before merging";

impl Header {
    pub fn text(&self, now: u64) -> Text {
        let repo = self.repo.to_string();
        let stale = match &self.kind {
            Kind::PullRequest {
                info:
                    Some(PrInfo {
                        saved: true,
                        fetched_at,
                        ..
                    }),
                ..
            } => Some(match fetched_at {
                Some(t) => format!("details from {}", ago(now, *t)),
                None => "saved details".to_string(),
            }),
            _ => None,
        };
        let (lead, title, author, state) = match &self.kind {
            Kind::PullRequest {
                number,
                info: Some(i),
            } => (
                format!("#{number}"),
                Some(i.title.clone()),
                Some(i.author.clone()),
                Some(i.state),
            ),
            Kind::PullRequest { number, info: None } => {
                (format!("pull request {number}"), None, None, None)
            }
            Kind::Branch(b) => (repo, Some(format!("branch {b}")), None, None),
            Kind::Commit(c) => (repo, Some(format!("commit {c}")), None, None),
            Kind::Other(w) => (repo, Some(w.clone()), None, None),
        };
        let behind = self.behind.filter(|n| *n > 0).map(|n| match &self.target {
            Some(t) => format!("{n} behind {t}"),
            None => format!("{n} behind"),
        });
        let freshness = match (self.offline, self.fetched_at) {
            (true, Some(t)) => Some(format!("offline \u{b7} last fetch {}", ago(now, t))),
            (true, None) => Some("offline".to_string()),
            (false, Some(t)) => Some(match &self.target {
                Some(b) => format!("{b} fetched {}", ago(now, t)),
                None => format!("fetched {}", ago(now, t)),
            }),
            (false, None) => None,
        };
        Text {
            lead,
            title,
            author,
            state,
            stale,
            behind,
            freshness,
        }
    }

    /// The pull request number, if this is one.
    pub fn pull_request(&self) -> Option<u64> {
        match self.kind {
            Kind::PullRequest { number, .. } => Some(number),
            _ => None,
        }
    }
}

// ---- "PR updated" ----

/// What a check of the pull request's head found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrUpdate {
    /// Not checked, or the host gave no head: no banner.
    Unknown,
    Same,
    /// The host's head differs: the short id to show.
    Newer(String),
}

/// Compares the head the worktree has with the host's. Either may be abbreviated (Bitbucket
/// Cloud reports short ids), so one being a prefix of the other counts as the same.
pub fn pr_update(worktree_head: &str, host_head: Option<&str>) -> PrUpdate {
    let Some(host) = host_head.map(str::trim).filter(|h| !h.is_empty()) else {
        return PrUpdate::Unknown;
    };
    let mine = worktree_head.trim();
    if mine.is_empty() {
        return PrUpdate::Unknown;
    }
    let (a, b) = (mine.to_ascii_lowercase(), host.to_ascii_lowercase());
    if a.starts_with(&b) || b.starts_with(&a) {
        PrUpdate::Same
    } else {
        PrUpdate::Newer(cho_git::short(host))
    }
}

// ---- Open on the host ----

/// "GitHub" or "Bitbucket"; `None` for a local repository.
pub fn host_label(repo: &RepoRef) -> Option<&'static str> {
    match repo {
        RepoRef::GitHub { .. } => Some("GitHub"),
        RepoRef::Bitbucket { .. } | RepoRef::BitbucketDc { .. } => Some("Bitbucket"),
        RepoRef::Local { .. } => None,
    }
}

/// The web page of `path` (relative to the worktree) at `line` on the side being read: After
/// is the head commit, Before the comparison's base.
pub fn web_link(
    repo: &RepoRef,
    before: bool,
    head: &str,
    base: Option<&str>,
    path: &str,
    line: Option<usize>,
) -> Option<String> {
    let rev = if before { base? } else { head };
    cho_core::editor::web_url(repo, rev, path, line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gh() -> RepoRef {
        RepoRef::parse("github:acme/api").unwrap()
    }

    fn header(kind: Kind) -> Header {
        Header {
            repo: gh(),
            kind,
            head: "a".repeat(40),
            target: Some("main".into()),
            behind: Some(0),
            fetched_at: Some(1000),
            offline: false,
        }
    }

    #[test]
    fn a_pull_request_with_details_shows_number_title_author_and_state() {
        let h = header(Kind::PullRequest {
            number: 123,
            info: Some(PrInfo {
                title: "Fix login".into(),
                author: "ada".into(),
                state: PrState::Merged,
                fetched_at: Some(1000),
                saved: true,
            }),
        });
        let t = h.text(1000 + 12 * 60);
        assert_eq!(t.lead, "#123");
        assert_eq!(t.title.as_deref(), Some("Fix login"));
        assert_eq!(t.author.as_deref(), Some("ada"));
        assert_eq!(t.state, Some(PrState::Merged));
        assert_eq!(t.freshness.as_deref(), Some("main fetched 12 min ago"));
        assert_eq!(t.stale.as_deref(), Some("details from 12 min ago"));
        assert_eq!(t.behind, None);
    }

    #[test]
    fn details_from_this_open_have_no_age_and_saved_ones_always_do() {
        let pr = |saved, fetched_at| {
            header(Kind::PullRequest {
                number: 1,
                info: Some(PrInfo {
                    title: "t".into(),
                    author: "a".into(),
                    state: PrState::Open,
                    fetched_at,
                    saved,
                }),
            })
        };
        assert_eq!(pr(false, Some(1000)).text(1000).stale, None);
        // No threshold: even seconds old is said.
        assert_eq!(
            pr(true, Some(1000)).text(1005).stale.as_deref(),
            Some("details from just now")
        );
        assert_eq!(
            pr(true, Some(1000)).text(1000 + 3 * 86400).stale.as_deref(),
            Some("details from 3 days ago")
        );
        assert_eq!(
            pr(true, None).text(5).stale.as_deref(),
            Some("saved details")
        );
    }

    #[test]
    fn a_pull_request_from_the_cache_alone_has_no_title_yet() {
        let t = header(Kind::PullRequest {
            number: 12,
            info: None,
        })
        .text(1000);
        assert_eq!(t.lead, "pull request 12");
        assert_eq!((t.title, t.author, t.state), (None, None, None));
    }

    #[test]
    fn a_branch_and_a_commit_name_the_project_and_the_target() {
        let t = header(Kind::Branch("feat/login".into())).text(1000);
        assert_eq!(t.lead, "github:acme/api");
        assert_eq!(t.title.as_deref(), Some("branch feat/login"));
        let t = header(Kind::Commit("3f9c1e2".into())).text(1000);
        assert_eq!(t.title.as_deref(), Some("commit 3f9c1e2"));
    }

    #[test]
    fn behind_shows_only_when_there_are_commits() {
        let mut h = header(Kind::Branch("x".into()));
        for none in [None, Some(0)] {
            h.behind = none;
            let t = h.text(1000);
            assert_eq!(t.behind, None);
        }
        h.behind = Some(3);
        let t = h.text(1000);
        assert_eq!(t.behind.as_deref(), Some("3 behind main"));
    }

    #[test]
    fn offline_says_so_with_the_last_fetch() {
        let mut h = header(Kind::Branch("x".into()));
        h.offline = true;
        let t = h.text(1000 + 2 * 3600);
        assert_eq!(
            t.freshness.as_deref(),
            Some("offline \u{b7} last fetch 2 h ago")
        );
        h.fetched_at = None;
        assert_eq!(h.text(5).freshness.as_deref(), Some("offline"));
        h.offline = false;
        assert_eq!(h.text(5).freshness, None);
    }

    #[test]
    fn the_banner_decision_compares_heads() {
        let full = "a71d004e0f1c2b3d4e5f60718293a4b5c6d7e8f9";
        assert_eq!(pr_update(full, None), PrUpdate::Unknown);
        assert_eq!(pr_update(full, Some("")), PrUpdate::Unknown);
        assert_eq!(pr_update("", Some(full)), PrUpdate::Unknown);
        assert_eq!(pr_update(full, Some(full)), PrUpdate::Same);
        assert_eq!(pr_update(full, Some("a71d004")), PrUpdate::Same);
        assert_eq!(pr_update("a71d004", Some(full)), PrUpdate::Same);
        assert_eq!(pr_update(full, Some("A71D004E0F1C")), PrUpdate::Same);
        assert_eq!(
            pr_update(full, Some("b00b1e5c0ffee0000000000000000000000000000")),
            PrUpdate::Newer("b00b1e5".into())
        );
        assert_eq!(
            pr_update(full, Some("b00b1e5")),
            PrUpdate::Newer("b00b1e5".into())
        );
    }

    #[test]
    fn the_host_button_names_the_host() {
        assert_eq!(host_label(&gh()), Some("GitHub"));
        let bb = RepoRef::parse("bitbucket:ws/repo").unwrap();
        assert_eq!(host_label(&bb), Some("Bitbucket"));
        let dc = RepoRef::parse("bitbucket-dc:git.example.com/PRJ/repo").unwrap();
        assert_eq!(host_label(&dc), Some("Bitbucket"));
        let local = RepoRef::Local {
            path: "/tmp/x".into(),
        };
        assert_eq!(host_label(&local), None);
    }

    #[test]
    fn the_web_link_is_at_the_commit_of_the_side_being_read() {
        let (head, base) = ("h".repeat(40), "b".repeat(40));
        let after = web_link(&gh(), false, &head, Some(&base), "src/a.rs", Some(7));
        assert_eq!(
            after.as_deref(),
            Some(format!("https://github.com/acme/api/blob/{head}/src/a.rs#L7").as_str())
        );
        let before = web_link(&gh(), true, &head, Some(&base), "src/a.rs", None);
        assert_eq!(
            before.as_deref(),
            Some(format!("https://github.com/acme/api/blob/{base}/src/a.rs").as_str())
        );
        assert_eq!(web_link(&gh(), true, &head, None, "src/a.rs", None), None);
        let bb = RepoRef::parse("bitbucket:ws/repo").unwrap();
        assert_eq!(
            web_link(&bb, false, "abc", None, "a b.rs", Some(2)).as_deref(),
            Some("https://bitbucket.org/ws/repo/src/abc/a%20b.rs#lines-2")
        );
    }
}
