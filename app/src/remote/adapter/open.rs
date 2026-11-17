//! Opening a remote project: the request, the open itself (with the `chorong://` link fallback),
//! the branches the form completes from, and the header band's facts.

use super::{Backend, build_sources, cache, real_backend, real_sources, require_cache};
use crate::remote::failure::{FailKind, Failure};
use crate::remote::header::{Header, Kind, PrInfo, PrState};
use cho_core::RepoRef;
use cho_sources::state::RepoState;
use cho_sources::{
    Cache, Cancel, ErrorKind, OpenRequest, OpenedAs, Project, Projects, Stage, TargetKind,
};
use std::path::{Path, PathBuf};

/// What to open.
#[derive(Debug, Clone)]
pub struct Request {
    pub repo: RepoRef,
    /// A clone URL the input gave (Data Center under a context path), used when the project
    /// is new.
    pub clone_url: Option<String>,
    pub kind: TargetKind,
    pub path: Option<String>,
    pub line: Option<usize>,
    /// Before override: a branch or commit.
    pub base: Option<String>,
}

/// An open that worked.
#[derive(Debug, Clone)]
pub struct Done {
    pub repo: String,
    pub worktree: PathBuf,
    /// Before, as a full commit id (the merge base, or the override).
    pub before: String,
    /// What Before was compared against: the target branch or the override.
    pub against: String,
    pub merge_bases: usize,
    pub path: Option<String>,
    pub line: Option<usize>,
    pub notes: Vec<String>,
    /// What the remote header band shows, from the open's own result.
    pub header: Header,
}

/// Branches already in the project's clone (nothing is fetched; empty before the first open).
pub fn cached_branches(repo: &RepoRef) -> Vec<String> {
    let Some(cache) = cache() else {
        return Vec::new();
    };
    let Ok(paths) = cache.repo(repo) else {
        return Vec::new();
    };
    branches_of(&paths.bare)
}

fn branches_of(bare: &Path) -> Vec<String> {
    if !bare.exists() {
        return Vec::new();
    }
    cho_git::list_refs(bare)
        .map(|refs| {
            refs.into_iter()
                .filter(|r| r.kind == cho_git::RefKind::Branch)
                .map(|r| r.short)
                .collect()
        })
        .unwrap_or_default()
}

/// The clone URL a repository name implies (HTTPS), to prefill the Data Center box.
pub fn derived_clone_url(repo: &RepoRef) -> Option<String> {
    cho_sources::hosts::clone_url(repo, &Project::default()).ok()
}

/// What the remote has, for completing a branch of a project with no clone yet.
#[derive(Debug, Clone, Default)]
pub struct RemoteHeads {
    pub default: Option<String>,
    pub branches: Vec<String>,
}

/// Asks the remote for its branches (one `git ls-remote`; nothing is cloned or locked). Blocks
/// (network): background executor only, once per project per opening of the form. `cancel`
/// stops it when the form closes.
pub fn remote_branches(
    repo: &RepoRef,
    clone_url: Option<&str>,
    cancel: &Cancel,
) -> Result<RemoteHeads, String> {
    let projects = Projects::open_default().map_err(|e| e.to_string())?;
    let project = projects.get(repo).cloned().unwrap_or_else(|| Project {
        url: clone_url.map(str::to_string),
        ..Project::default()
    });
    let sources = real_sources(require_cache()?, credential_helper());
    let heads = sources
        .remote_branches_with(repo, &project, cancel)
        .map_err(|e| e.message)?;
    Ok(RemoteHeads {
        default: heads.default,
        branches: heads.branches.into_iter().map(|(n, _)| n).collect(),
    })
}

/// Said once per run when Chorong's token fallback is unavailable.
const NO_HELPER: &str =
    "Chorong's token fallback needs the `cho` command installed next to the viewer";

/// The `cho` command-line program, which git calls as `cho credential` when Chorong holds a
/// token for the host. The helper receives tokens, so it is only ever the program installed
/// next to this one (canonicalised, a regular executable file), never one found on `PATH`.
fn credential_helper() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    helper_beside(exe.parent()?)
}

/// The `cho` program in `dir`, if it is a regular executable file.
fn helper_beside(dir: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) { "cho.exe" } else { "cho" };
    let path = dir.join(name).canonicalize().ok()?;
    // The canonical path must still be in `dir` (no symbolic link out of the install folder).
    if path.parent() != Some(dir) {
        return None;
    }
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    Some(path)
}

/// Whether the missing-helper note was already shown in this run.
static HELPER_NOTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Opens `req`: adds the project if it is new (pressing Open is consent), then clones,
/// fetches and places the worktree. Blocks, possibly for minutes the first time. `progress`
/// is told where the open is (as a short phrase); raising `cancel` stops it, which comes back
/// as `FailKind::Cancelled`.
pub fn open(req: &Request, progress: &dyn Fn(String), cancel: &Cancel) -> Result<Done, Failure> {
    let mut projects = Projects::open_default().map_err(|e| Failure::other(e.to_string()))?;
    let cache = require_cache().map_err(Failure::other)?;
    let helper = credential_helper();
    let mut done = open_in(
        cache,
        &mut projects,
        real_backend(helper.clone()),
        req,
        progress,
        cancel,
    )?;
    if helper.is_none() && !HELPER_NOTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        done.notes.push(NO_HELPER.into());
    }
    // One recent.json for the viewer and `cho open`: links to this repository resolve here.
    let _ = cho_sources::recent::remember(&done.worktree);
    Ok(done)
}

/// Opens like [`open`] for a `chorong://` link that names a Before (`req.base`): if the host
/// and cache don't have that commit, opens again with the merge base and says so in the notes.
/// Anything else that fails fails as it does in [`open`].
pub fn open_link(
    req: &Request,
    progress: &dyn Fn(String),
    cancel: &Cancel,
) -> Result<Done, Failure> {
    let Some(base) = req.base.clone() else {
        return open(req, progress, cancel);
    };
    match open(req, progress, cancel) {
        Err(f) if base_is_missing(&f, &base) => {
            let without = Request {
                base: None,
                ..req.clone()
            };
            let mut done = open(&without, progress, cancel)?;
            done.notes.insert(0, crate::links::MISSING_BASE.to_string());
            Ok(done)
        }
        Ok(mut done) => {
            // The notice names the commit, not the 40 digits the link carried.
            done.against = cho_git::short(&base);
            Ok(done)
        }
        other => other,
    }
}

/// True when `f` says the commit `base` does not exist (rather than anything else going
/// wrong), so the open can go on without it.
fn base_is_missing(f: &Failure, base: &str) -> bool {
    matches!(&f.kind, FailKind::CommitNotFound { commit } if commit.eq_ignore_ascii_case(base))
}

/// The cache worktree of pull request `number` of `repo`, if Chorong has one (nothing is
/// fetched). Reads the cache's state file: background executor, not the UI thread.
pub fn pr_worktree(repo: &RepoRef, number: u64) -> Option<PathBuf> {
    pr_worktree_in(&cache()?, repo, number)
}

fn pr_worktree_in(cache: &Cache, repo: &RepoRef, number: u64) -> Option<PathBuf> {
    let paths = cache.repo(repo).ok()?;
    let slug = cho_sources::paths::pr_slug(number);
    let state = RepoState::load(&paths.state);
    state.worktrees.contains_key(&slug).then_some(())?;
    Some(paths.worktree(&slug)).filter(|p| p.is_dir())
}

/// The backend's error as the form needs it.
fn failure(e: cho_sources::Error) -> Failure {
    let kind = match e.kind {
        ErrorKind::Cancelled => FailKind::Cancelled,
        ErrorKind::Offline => FailKind::Offline,
        ErrorKind::Auth { host } => FailKind::Auth { host },
        ErrorKind::NotFound => FailKind::NotFound,
        ErrorKind::CommitNotFound { commit } => FailKind::CommitNotFound { commit },
        ErrorKind::Ambiguous { name } => FailKind::Ambiguous {
            branch: name.clone(),
            commit: name,
        },
        ErrorKind::LockBusy => FailKind::LockBusy,
        ErrorKind::Unsupported | ErrorKind::Other => FailKind::Other,
    };
    Failure {
        kind,
        message: e.message,
        detail: e.detail,
    }
}

/// Where an open is, in words for the progress row and the status bar.
pub fn stage_text(stage: &Stage) -> String {
    let with = |what: &str, p: &Option<cho_sources::GitProgress>| match p {
        Some(p) => format!("{what} ({} {}%)", p.phase.to_lowercase(), p.percent),
        None => what.to_string(),
    };
    match stage {
        Stage::Lock => "waiting for the repository lock".into(),
        Stage::Clone { progress } => with("cloning", progress),
        Stage::ListRemote => "listing the remote's branches".into(),
        Stage::PullRequest => "reading the pull request".into(),
        Stage::Fetch { progress } => with("fetching", progress),
        Stage::MergeBase => "finding the merge base".into(),
        Stage::Prefetch { progress } => with("fetching Before's files", progress),
        Stage::Worktree { progress } => with("placing the worktree", progress),
    }
}

pub(super) fn open_in(
    cache: Cache,
    projects: &mut Projects,
    backend: Backend<'_>,
    req: &Request,
    progress: &dyn Fn(String),
    cancel: &Cancel,
) -> Result<Done, Failure> {
    let (project, added) = projects
        .get_or_add(&req.repo, req.clone_url.clone())
        .map_err(|e| Failure::other(e.to_string()))?;
    let mut notes: Vec<String> = added.into_iter().collect();
    let sources = build_sources(cache, backend);
    // Sources drops the indexes of evicted worktrees itself (`index_cache`).
    let opened = sources
        .open_with(
            &req.repo,
            &project,
            &OpenRequest {
                kind: req.kind.clone(),
                path: req.path.clone(),
                line: req.line,
                base: req.base.clone(),
            },
            &|stage| progress(stage_text(&stage)),
            cancel,
        )
        .map_err(failure)?;
    notes.extend(opened.notes.iter().cloned());
    Ok(Done {
        repo: opened.repo.clone(),
        worktree: opened.worktree.clone(),
        before: opened.before.id.clone(),
        against: req
            .base
            .clone()
            .unwrap_or_else(|| opened.target_branch.clone()),
        merge_bases: opened.merge_bases,
        path: opened.path.clone(),
        line: opened.line,
        notes,
        header: header_of(&opened),
    })
}

fn pr_state(s: cho_sources::PrState) -> PrState {
    match s {
        cho_sources::PrState::Open => PrState::Open,
        cho_sources::PrState::Merged => PrState::Merged,
        cho_sources::PrState::Declined => PrState::Declined,
    }
}

/// The band's kind for what was opened, with the pull request's details when there are any.
fn kind_of(opened_as: &OpenedAs, info: Option<PrInfo>) -> Kind {
    match opened_as {
        OpenedAs::PullRequest(number) => Kind::PullRequest {
            number: *number,
            info,
        },
        OpenedAs::Branch(b) => Kind::Branch(b.clone()),
        OpenedAs::Commit(c) => Kind::Commit(cho_git::short(c)),
    }
}

fn pr_info(details: &cho_sources::PrDetails, fetched_at: u64, saved: bool) -> PrInfo {
    PrInfo {
        title: details.title.clone(),
        author: details.author.clone(),
        state: pr_state(details.state),
        fetched_at: Some(fetched_at),
        saved,
    }
}

/// The band's data straight from an open (no cache read needed).
fn header_of(o: &cho_sources::Opened) -> Header {
    let info = o
        .pull_request
        .as_ref()
        .map(|p| pr_info(&p.details, p.fetched_at, p.stale));
    Header {
        repo: RepoRef::parse(&o.repo).unwrap_or_else(|_| RepoRef::Local {
            path: o.repo.clone(),
        }),
        kind: kind_of(&o.opened_as, info),
        head: o.head.id.clone(),
        target: Some(o.target_branch.clone()),
        behind: Some(o.behind),
        fetched_at: o.target_fetched_at,
        offline: o.offline,
    }
}

/// The band's data for a window opened later (from recents or a link): what the cache
/// recorded about the worktree, and how far behind the default branch it is. Reads the
/// object store, so it runs on the background executor. `None` for a folder that is not a
/// cache worktree.
pub fn read_header(folder: &Path) -> Option<Header> {
    read_header_in(&cache()?, folder)
}

fn read_header_in(cache: &Cache, folder: &Path) -> Option<Header> {
    let (repo, slug) = cache.worktree_owner(folder)?;
    let paths = cache.repo(&repo).ok()?;
    let state = RepoState::load(&paths.state);
    let wt = state.worktrees.get(&slug)?;
    // A pull request's details are saved with its worktree.
    let mut target = state.default_branch.clone();
    let kind = match &wt.opened_as {
        Some(a) => {
            let saved = wt
                .pull_request
                .as_ref()
                .filter(|_| a.pull_request().is_some());
            if let Some(rec) = saved {
                target = Some(rec.details.target_branch.clone());
            }
            kind_of(a, saved.map(|r| pr_info(&r.details, r.fetched_at, true)))
        }
        None => Kind::Other(wt.what.clone()),
    };
    let target_ref = target.as_ref().map(|t| format!("refs/heads/{t}"));
    let fetched_at = target_ref
        .as_ref()
        .and_then(|r| state.fetched.get(r))
        .copied();
    let behind = target_ref
        .and_then(|r| cho_sources::reads::resolve_commit(&paths.bare, &r))
        .and_then(|tip| cho_sources::reads::count_missing(&paths.bare, &wt.head, &tip).ok());
    Some(Header {
        repo,
        kind,
        head: wt.head.clone(),
        target,
        behind,
        fetched_at,
        // Whether the last fetch failed is not recorded; only an open knows.
        offline: false,
    })
}

/// Why asking the host for a pull request's head failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadError {
    /// The host could not be reached (the band says "offline" until a check works).
    pub offline: bool,
    pub message: String,
}

/// The pull request's head as the host reports it right now (cheap, no REST quota), for the
/// "PR updated" banner. `Ok(None)` means not checked (no cache directory). Blocks (network):
/// background executor only.
pub fn pr_head(repo: &RepoRef, number: u64) -> Result<Option<cho_sources::PrHead>, HeadError> {
    let plain = |message: String| HeadError {
        offline: false,
        message,
    };
    let projects = Projects::open_default().map_err(|e| plain(e.to_string()))?;
    let project = projects.get(repo).cloned().unwrap_or_default();
    let Some(cache) = cache() else {
        return Ok(None);
    };
    let sources = real_sources(cache, credential_helper());
    sources
        .pr_head(repo, &project, number)
        .map(Some)
        .map_err(|e| HeadError {
            offline: e.kind == ErrorKind::Offline,
            message: e.message,
        })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use cho_core::test_repo::git;

    #[test]
    fn the_credential_helper_is_only_the_cho_beside_the_app() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::create_dir_all(root.join("other")).unwrap();
        let dir = root.join("app").canonicalize().unwrap();
        // Nothing next to the app: no helper (PATH is never searched).
        assert_eq!(helper_beside(&dir), None);
        let name = if cfg!(windows) { "cho.exe" } else { "cho" };
        let cho = dir.join(name);
        std::fs::write(&cho, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Present but not executable: no helper.
            std::fs::set_permissions(&cho, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(helper_beside(&dir), None);
            std::fs::set_permissions(&cho, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(helper_beside(&dir), Some(cho.clone()));
        // A directory named cho is not a helper.
        let other_dir = root.join("other").canonicalize().unwrap();
        std::fs::create_dir(other_dir.join(name)).unwrap();
        assert_eq!(helper_beside(&other_dir), None);
    }

    #[test]
    fn only_a_missing_base_commit_lets_a_link_open_without_it() {
        let base = "0123456789abcdef0123456789abcdef01234567";
        let f = |kind| Failure {
            kind,
            message: "whatever the backend says".into(),
            detail: None,
        };
        let missing = |c: &str| {
            f(FailKind::CommitNotFound {
                commit: c.to_string(),
            })
        };
        assert!(base_is_missing(&missing(base), base));
        // The backend lowercases the id; a link may carry it in upper case.
        assert!(base_is_missing(&missing(base), &base.to_ascii_uppercase()));
        // Another commit, a missing pull request or another kind of failure still fails.
        assert!(!base_is_missing(&missing("ffff"), base));
        assert!(!base_is_missing(&f(FailKind::NotFound), base));
        assert!(!base_is_missing(&f(FailKind::Offline), base));
        // The backend's own error comes through as that kind.
        let e = cho_sources::Error::new(
            ErrorKind::CommitNotFound {
                commit: base.into(),
            },
            "x",
        );
        assert!(base_is_missing(&failure(e), base));
    }

    #[test]
    fn a_pull_requests_cache_worktree_is_found_only_when_it_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cache = Cache::at(root);
        let repo = RepoRef::parse("github:acme/api").unwrap();
        assert_eq!(pr_worktree_in(&cache, &repo, 12), None);
        let paths = cache.repo(&repo).unwrap();
        let mut state = RepoState::default();
        state
            .worktrees
            .insert("pr-12".into(), cho_sources::state::WorktreeState::default());
        std::fs::create_dir_all(&paths.dir).unwrap();
        state.save(&paths.state).unwrap();
        // Recorded but the folder is gone: not found.
        assert_eq!(pr_worktree_in(&cache, &repo, 12), None);
        std::fs::create_dir_all(paths.worktree("pr-12")).unwrap();
        assert_eq!(
            pr_worktree_in(&cache, &repo, 12),
            Some(paths.worktree("pr-12"))
        );
        // Another pull request, and a local folder, have none.
        assert_eq!(pr_worktree_in(&cache, &repo, 13), None);
        assert_eq!(
            pr_worktree_in(&cache, &RepoRef::Local { path: "/x".into() }, 12),
            None
        );
    }

    /// A pull request whose details couldn't be read showed in the band as a commit; it is
    /// a pull request with its number.
    #[test]
    fn a_pull_request_without_details_is_still_a_pull_request() {
        assert_eq!(
            kind_of(&OpenedAs::PullRequest(12), None),
            Kind::PullRequest {
                number: 12,
                info: None
            }
        );
        let full = "3f9c1e2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert_eq!(
            kind_of(&OpenedAs::Commit(full.into()), None),
            Kind::Commit("3f9c1e2".into())
        );
    }

    /// A worktree recorded before the typed record existed is read from its words; one
    /// whose words this version doesn't know shows them as they are.
    #[test]
    fn old_cache_records_still_name_what_was_opened() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cache = Cache::at(root);
        let repo = RepoRef::parse("github:acme/api").unwrap();
        let paths = cache.repo(&repo).unwrap();
        std::fs::create_dir_all(&paths.dir).unwrap();
        std::fs::write(
            &paths.state,
            r#"{"worktrees":{
                "pr-12":{"seq":1,"opened":0,"head":"","what":"pull request 12"},
                "x":{"seq":2,"opened":0,"head":"","what":"tag v1"}}}"#,
        )
        .unwrap();
        for s in ["pr-12", "x"] {
            std::fs::create_dir_all(paths.worktree(s)).unwrap();
        }
        let kind = |s: &str| read_header_in(&cache, &paths.worktree(s)).unwrap().kind;
        assert_eq!(
            kind("pr-12"),
            Kind::PullRequest {
                number: 12,
                info: None
            }
        );
        assert_eq!(kind("x"), Kind::Other("tag v1".into()));
    }

    /// A bare repository on disk standing in for the remote: `main` with one commit and
    /// `feature` one commit ahead of it. Returns (remote url, main's commit).
    fn remote(dir: &Path) -> (String, String) {
        let work = dir.join("work");
        let bare = dir.join("remote.git");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        std::fs::write(work.join("a.txt"), "one\n").unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-q", "-m", "one"]);
        let main = git(&work, &["rev-parse", "HEAD"]);
        git(&work, &["switch", "-q", "-c", "feature"]);
        std::fs::write(work.join("a.txt"), "two\n").unwrap();
        git(&work, &["commit", "-q", "-am", "two"]);
        git(
            dir,
            &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
        );
        git(
            &work,
            &["push", "-q", bare.to_str().unwrap(), "main", "feature"],
        );
        (format!("file://{}", bare.display()), main)
    }

    #[test]
    fn opens_a_branch_of_a_local_remote_and_adds_the_project() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let (url, main) = remote(dir);
        let cache = Cache::at(dir.join("cache"));
        let mut projects = Projects::open(dir.join("projects.json")).unwrap();

        let req = request(&url, TargetKind::Branch("feature".into()));
        let done = open_in(
            cache.clone(),
            &mut projects,
            backend(),
            &req,
            &|_| {},
            &Cancel::new(),
        )
        .unwrap();
        assert!(done.worktree.join("a.txt").is_file());
        assert_eq!(
            done.before, main,
            "Before is the merge base with the default branch"
        );
        assert_eq!(done.against, "main");
        assert_eq!(
            (done.path.as_deref(), done.line),
            (Some("src/a.rs"), Some(3))
        );
        assert_eq!(done.notes[0], "Added github:acme/api to your projects.");
        assert_eq!(done.header.kind, Kind::Branch("feature".into()));
        assert_eq!(done.header.target.as_deref(), Some("main"));
        assert_eq!(done.header.behind, Some(0));
        assert!(done.header.fetched_at.is_some() && !done.header.offline);
        // A window opened later reads the same from the cache alone.
        let later = read_header_in(&cache, &done.worktree).expect("a cache worktree");
        assert_eq!(later.kind, done.header.kind);
        assert_eq!(later.head, done.header.head);
        assert_eq!(later.behind, Some(0));
        assert_eq!(later.fetched_at, done.header.fetched_at);
        assert!(read_header_in(&cache, dir).is_none(), "not a cache folder");
        assert!(projects.get(&req.repo).is_some());
        assert_eq!(
            cache.worktree_owner(&done.worktree).map(|o| o.0),
            Some(req.repo.clone())
        );
        assert_eq!(cached_branches_in(&cache, &req.repo), ["feature", "main"]);

        // The project is known now: no second note, and a base override is what Before uses.
        let mut again = request(&url, TargetKind::Branch("feature".into()));
        again.base = Some("main".into());
        let done = open_in(
            cache,
            &mut projects,
            backend(),
            &again,
            &|_| {},
            &Cancel::new(),
        )
        .unwrap();
        assert!(done.notes.iter().all(|n| !n.starts_with("Added")));
        assert_eq!(done.against, "main");

        // A branch the remote does not have is an error sentence, not a panic.
        let missing = request(&url, TargetKind::Branch("nope".into()));
        let cache = Cache::at(dir.join("cache"));
        let seen = std::cell::RefCell::new(Vec::new());
        let failed = open_in(
            cache.clone(),
            &mut projects,
            backend(),
            &missing,
            &|s| seen.borrow_mut().push(s),
            &Cancel::new(),
        )
        .unwrap_err();
        assert_eq!(failed.kind, FailKind::NotFound, "{failed:?}");
        assert!(
            seen.borrow()
                .iter()
                .any(|s| s.contains("remote's branches")),
            "stages reach the callback: {:?}",
            seen.borrow()
        );

        // A cancel that is already raised stops the open, silently.
        let cancel = Cancel::new();
        cancel.cancel();
        let stopped = open_in(
            cache,
            &mut projects,
            backend(),
            &request(&url, TargetKind::Branch("feature".into())),
            &|_| {},
            &cancel,
        )
        .unwrap_err();
        assert_eq!(stopped.kind, FailKind::Cancelled, "{stopped:?}");
        assert_eq!(stopped.shown(), None);
    }

    #[test]
    fn stages_read_as_short_phrases() {
        use cho_sources::GitProgress;
        assert_eq!(stage_text(&Stage::Lock), "waiting for the repository lock");
        assert_eq!(
            stage_text(&Stage::Clone {
                progress: Some(GitProgress {
                    phase: "Receiving objects".into(),
                    percent: 45
                })
            }),
            "cloning (receiving objects 45%)"
        );
        assert_eq!(stage_text(&Stage::Fetch { progress: None }), "fetching");
        assert_eq!(stage_text(&Stage::MergeBase), "finding the merge base");
    }

    fn cached_branches_in(cache: &Cache, repo: &RepoRef) -> Vec<String> {
        branches_of(&cache.repo(repo).unwrap().bare)
    }
}
