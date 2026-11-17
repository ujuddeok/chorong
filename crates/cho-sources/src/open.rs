//! Opening a branch, pull request or commit of a remote repository: clone (once), fetch
//! what is needed, resolve the target, compute Before (the merge base with the target branch,
//! or an override) and place a detached worktree at After.

use crate::auth::{Host, Tokens};
use crate::gitcmd::{Git, GitFailure};
use crate::hosts::{self, Api, PrDetails, RateLimit};
use crate::paths::{Cache, RepoPaths, branch_slug, pr_slug, rev_slug};
use crate::progress::{Cancel, GitProgress, Stage};
use crate::projects::Project;
use crate::reads::{self, CommitInfo};
use crate::state::{OpenedAs, PrRecord, RepoState, WorktreeState};
use crate::target::{RefPathMatch, TargetKind, ambiguous, is_hex_commit, resolve_ref_path};
use crate::{Error, ErrorKind, Result, now_unix};
use cho_core::RepoRef;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Everything opening needs: where the cache is, how to run git, tokens and REST endpoints.
pub struct Sources<'a> {
    pub cache: Cache,
    pub git: Git,
    pub tokens: Tokens<'a>,
    pub api: Api,
    /// How long to wait for another open of the same repository before giving up.
    pub lock_timeout: std::time::Duration,
    /// The semantic index cache, so removed worktrees lose their indexes too (`None`: leave
    /// indexes alone).
    pub index_cache: Option<cho_semantic::IndexCache>,
}

/// A pull request's current head, for the "pull request updated" check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrHead {
    /// The full commit id; for Bitbucket Cloud, the host's abbreviated id when that commit
    /// hasn't been fetched yet.
    pub id: String,
    pub abbreviated: bool,
}

impl PrHead {
    /// True when the worktree's commit `full` is this head.
    pub fn matches(&self, full: &str) -> bool {
        !self.id.is_empty() && full.starts_with(&self.id)
    }
}

/// What [`Sources::remove_project`] removed.
#[derive(Debug, Clone, Serialize)]
pub struct RemovedProject {
    /// It was in `projects.json`.
    pub was_project: bool,
    /// Worktree folders removed (their indexes went too).
    pub worktrees: Vec<PathBuf>,
}

/// The default [`Sources::lock_timeout`].
pub const LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// What to open.
#[derive(Debug, Clone)]
pub struct OpenRequest {
    pub kind: TargetKind,
    pub path: Option<String>,
    pub line: Option<usize>,
    /// Before override: a branch or commit. Default: the merge base with the target branch.
    pub base: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BaseSource {
    MergeBase,
    Override,
}

/// The result of opening.
#[derive(Debug, Clone, Serialize)]
pub struct Opened {
    pub repo: String,
    /// The worktree, checked out (detached) at `head`.
    pub worktree: PathBuf,
    pub slug: String,
    /// What was opened. In JSON: `kind` (`branch`, `pull-request` or `commit`) and `branch`.
    #[serde(flatten, serialize_with = "opened_as_json")]
    pub opened_as: OpenedAs,
    /// The pull request's details, when it is one and they could be read (or a saved copy
    /// stood in). In JSON: `pull_request`, `pull_request_fetched_at` and
    /// `pull_request_stale`.
    #[serde(flatten, serialize_with = "pr_snapshot_json")]
    pub pull_request: Option<PrSnapshot>,
    /// After.
    pub head: CommitInfo,
    /// Before.
    pub before: CommitInfo,
    pub base_source: BaseSource,
    /// How many merge bases there were (Chorong picks one by a fixed rule when there are
    /// several: newest committer time, then lowest id).
    pub merge_bases: usize,
    /// The branch the head is compared with: the pull request's target, else the default
    /// branch.
    pub target_branch: String,
    pub target: CommitInfo,
    /// Commits on the target branch that the head doesn't have.
    pub behind: usize,
    /// Commits on the head that the target branch doesn't have.
    pub ahead: usize,
    /// When the target branch was last fetched (seconds since the Unix epoch).
    pub target_fetched_at: Option<u64>,
    /// The fetch failed and the last fetched refs were used.
    pub offline: bool,
    pub path: Option<String>,
    pub line: Option<usize>,
    pub notes: Vec<String>,
    /// Worktrees removed to stay within the project's cap (their indexes are dropped too).
    pub evicted: Vec<PathBuf>,
    pub rate_limit: Option<RateLimit>,
}

/// A pull request's details as this open has them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrSnapshot {
    pub details: PrDetails,
    /// When the details were read from the host (seconds since the Unix epoch).
    pub fetched_at: u64,
    /// The host couldn't be reached and these are the details saved with the worktree.
    pub stale: bool,
}

/// [`Opened::opened_as`] as `cho open --json` has always written it.
fn opened_as_json<S: serde::Serializer>(
    a: &OpenedAs,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Json<'a> {
        kind: &'static str,
        branch: Option<&'a str>,
    }
    Json {
        kind: a.kind(),
        branch: a.branch(),
    }
    .serialize(s)
}

/// [`Opened::pull_request`] as `cho open --json` has always written it.
fn pr_snapshot_json<S: serde::Serializer>(
    pr: &Option<PrSnapshot>,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Json<'a> {
        pull_request: Option<&'a PrDetails>,
        pull_request_fetched_at: Option<u64>,
        pull_request_stale: bool,
    }
    Json {
        pull_request: pr.as_ref().map(|p| &p.details),
        pull_request_fetched_at: pr.as_ref().map(|p| p.fetched_at),
        pull_request_stale: pr.as_ref().is_some_and(|p| p.stale),
    }
    .serialize(s)
}

/// The remote's default branch and branches (name → commit).
struct Heads {
    default: Option<String>,
    branches: Vec<(String, String)>,
}

impl Heads {
    fn has(&self, b: &str) -> bool {
        self.branches.iter().any(|(n, _)| n == b)
    }
    fn names(&self) -> Vec<String> {
        self.branches.iter().map(|(n, _)| n.clone()).collect()
    }
}

/// `git ls-remote --symref … HEAD refs/heads/*` output: the default branch and branches,
/// sorted by name.
fn parse_ls_remote(out: &str) -> Heads {
    let mut heads = Heads {
        default: None,
        branches: vec![],
    };
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("ref: ")
            && let Some((target, name)) = rest.split_once('\t')
            && name == "HEAD"
        {
            heads.default = target.strip_prefix("refs/heads/").map(str::to_string);
        } else if let Some((sha, name)) = line.split_once('\t')
            && let Some(b) = name.strip_prefix("refs/heads/")
        {
            heads.branches.push((b.to_string(), sha.to_string()));
        }
    }
    heads.branches.sort();
    heads
}

/// A remote's branches, for completion in the open form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteHeads {
    /// The remote's default branch (its `HEAD`).
    pub default: Option<String>,
    /// Branch name and the commit it points at, sorted by name.
    pub branches: Vec<(String, gix::ObjectId)>,
}

/// The host a clone URL points at: `https://h:443/…`, `ssh://git@h:7999/…` or `git@h:path`
/// → `h`. `None` for local (`file://`) URLs.
pub fn url_host(url: &str) -> Option<String> {
    let authority = if let Some((scheme, rest)) = url.split_once("://") {
        if scheme == "file" {
            return None;
        }
        rest.split('/').next()?
    } else {
        // scp-like `user@host:path`; a `/` before the first `:` makes it a local path.
        url.split_once(':')
            .map(|(a, _)| a)
            .filter(|a| !a.contains('/'))?
    };
    let host_port = authority.rsplit('@').next()?;
    let host = match host_port.strip_prefix('[') {
        // [::1]:8080
        Some(v6) => v6.split(']').next()?,
        None => host_port.split(':').next()?,
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// `acme/api` for `github:acme/api`: the repository as its host names it.
fn repo_path(r: &RepoRef) -> String {
    let s = r.to_string();
    s.split_once(':').map(|(_, p)| p.to_string()).unwrap_or(s)
}

/// The head side, once resolved.
enum Head {
    Branch(String),
    Pr {
        number: u64,
        /// Local ref the pull request's head is fetched into.
        local: String,
        /// What is fetched into it.
        remote: String,
    },
    Commit(String),
}

/// A full ref name (`refs/heads/feat/x`, `refs/pull/3/head`) safe to put in a refspec.
fn check_ref(r: &str) -> Result<()> {
    match r.strip_prefix("refs/") {
        Some(rest) if rest.contains('/') => crate::target::check_branch_name(rest)
            .map_err(|_| Error::msg(format!("`{r}` is not a valid ref name"))),
        _ => Err(Error::msg(format!("`{r}` is not a valid ref name"))),
    }
}

fn branch_ref(b: &str) -> String {
    format!("refs/heads/{b}")
}

fn spec(remote: &str, local: &str) -> String {
    format!("+{remote}:{local}")
}

impl Sources<'_> {
    fn host(repo: &RepoRef) -> Result<Host> {
        Host::of(repo)
            .ok_or_else(|| Error::unsupported(format!("`{repo}` is not a remote repository")))
    }

    /// Runs a network git command with the user's own setup; when that fails for want of a
    /// credential and Chorong has a token for the host, once more with Chorong's helper.
    /// Reports git's progress lines and stops when `cancel` is set.
    fn network(
        &self,
        dir: Option<&Path>,
        args: &[&str],
        host: &Host,
        on: &dyn Fn(GitProgress),
        cancel: &Cancel,
    ) -> std::result::Result<String, GitFailure> {
        match self.git.run_watched(dir, args, false, on, cancel) {
            Err(f)
                if !f.cancelled
                    && f.is_auth()
                    && self.git.credential_helper.is_some()
                    && self.tokens.get(host).is_some() =>
            {
                self.git.run_watched(dir, args, true, on, cancel)
            }
            r => r,
        }
    }

    /// The typed error for a failed network command to `url`, with a hint when a token may
    /// help. The message names the host Chorong actually tried (the clone URL's host, which a
    /// mirror or a Data Center URL can make differ from the repository's identity), and
    /// the repository too when they differ.
    fn auth_hint_for(&self, host: &Host, repo: &RepoRef, url: &str, f: &GitFailure) -> Error {
        let tried = url_host(url).unwrap_or_else(|| host.name().to_string());
        let elsewhere = tried != host.name();
        let mut e = f.to_error(&tried);
        // How the repository is named: `acme/api` on its own host, the full identity when
        // the host tried is another one.
        let name = if elsewhere {
            repo.to_string()
        } else {
            repo_path(repo)
        };
        let what = format!(" for {name}");
        match e.kind {
            ErrorKind::Offline if elsewhere => e.message.push_str(&what),
            ErrorKind::Auth { .. } if self.tokens.get(host).is_none() => {
                e.message = format!(
                    "{tried} needs a token or SSH key{what}: set up git or SSH access, or run \
                     `cho auth add {tried}`"
                );
            }
            ErrorKind::Auth { .. } => {
                e.message = format!(
                    "{tried} refused Chorong's token{what}; check it with `cho auth status {tried}`"
                );
            }
            ErrorKind::NotFound => {
                e.message = format!("{name} not found on {tried}");
            }
            _ => {}
        }
        e
    }

    /// Clones `url` into the cache once (bare, blobless).
    /// A cancelled or failed clone leaves nothing behind (the temporary folder is removed).
    fn ensure_clone(
        &self,
        repo: &RepoRef,
        paths: &RepoPaths,
        url: &str,
        host: &Host,
        progress: &dyn Fn(Stage),
        cancel: &Cancel,
    ) -> Result<bool> {
        if paths.bare.join("HEAD").exists() {
            return Ok(false);
        }
        self.cache.ensure_inside(&paths.bare)?;
        let parent = paths.bare.parent().expect("bare has a parent");
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::msg(format!("cannot create {}: {e}", parent.display())))?;
        let tmp = parent.join(format!(
            ".{}.clone-{}",
            paths.bare.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let tmp_s = tmp.to_string_lossy().into_owned();
        progress(Stage::Clone { progress: None });
        let r = self.network(
            Some(parent),
            &[
                "clone",
                "--bare",
                "--filter=blob:none",
                "--no-tags",
                "--no-recurse-submodules",
                "--progress",
                "--",
                url,
                &tmp_s,
            ],
            host,
            &|p| progress(Stage::Clone { progress: Some(p) }),
            cancel,
        );
        if let Err(f) = r {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(self.auth_hint_for(host, repo, url, &f));
        }
        std::fs::rename(&tmp, &paths.bare).map_err(|e| {
            let _ = std::fs::remove_dir_all(&tmp);
            Error::msg(format!("cannot move the clone into place: {e}"))
        })?;
        Ok(true)
    }

    fn remote_heads(
        &self,
        bare: &Path,
        host: &Host,
        cancel: &Cancel,
    ) -> std::result::Result<Heads, GitFailure> {
        let out = self.network(
            Some(bare),
            &[
                "ls-remote",
                "--symref",
                "--",
                "origin",
                "HEAD",
                "refs/heads/*",
            ],
            host,
            &|_| {},
            cancel,
        )?;
        Ok(parse_ls_remote(&out))
    }

    /// The remote's default branch and branches (name → commit), straight from the remote
    /// (`git ls-remote`), for branch completion before anything is cloned. Takes no lock
    /// and writes nothing; works without a clone. Stopped by `cancel`.
    pub fn remote_branches_with(
        &self,
        repo: &RepoRef,
        project: &Project,
        cancel: &Cancel,
    ) -> Result<RemoteHeads> {
        let host = Self::host(repo)?;
        let paths = self.cache.repo(repo)?;
        let url = hosts::clone_url(repo, project)?;
        cho_core::git::check_remote_url(&url).map_err(Error::msg)?;
        let dir = self.network_dir(&paths)?;
        let out = self
            .network(
                Some(&dir),
                &["ls-remote", "--symref", "--", &url, "HEAD", "refs/heads/*"],
                &host,
                &|_| {},
                cancel,
            )
            .map_err(|f| self.auth_hint_for(&host, repo, &url, &f))?;
        let heads = parse_ls_remote(&out);
        Ok(RemoteHeads {
            default: heads.default,
            branches: heads
                .branches
                .into_iter()
                .filter_map(|(n, id)| Some((n, gix::ObjectId::from_hex(id.as_bytes()).ok()?)))
                .collect(),
        })
    }

    /// Where to run a git command that needs no repository: the bare clone when there is
    /// one, else the cache root (created).
    fn network_dir(&self, paths: &RepoPaths) -> Result<PathBuf> {
        if paths.bare.exists() {
            return Ok(paths.bare.clone());
        }
        std::fs::create_dir_all(self.cache.root()).map_err(|e| {
            Error::msg(format!(
                "cannot create {}: {e}",
                self.cache.root().display()
            ))
        })?;
        Ok(self.cache.root().to_path_buf())
    }

    /// The last fetched heads, for when the remote can't be reached.
    fn local_heads(&self, bare: &Path, state: &RepoState) -> Heads {
        let branches = self
            .git
            .run(
                Some(bare),
                &[
                    "for-each-ref",
                    "--format=%(refname:short)\t%(objectname)",
                    "refs/heads/",
                ],
            )
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(n, s)| (n.to_string(), s.to_string()))
            .collect();
        let default = state.default_branch.clone().or_else(|| {
            self.git
                .run(Some(bare), &["symbolic-ref", "--quiet", "--short", "HEAD"])
                .ok()
        });
        Heads { default, branches }
    }

    /// Pull-request details over REST, with the host's token when there is one.
    pub fn pull_request(
        &self,
        repo: &RepoRef,
        project: Option<&Project>,
        number: u64,
    ) -> Result<(PrDetails, Option<RateLimit>)> {
        let host = Self::host(repo)?;
        let cred = self.tokens.get(&host);
        hosts::pull_request(&self.api, repo, project, number, cred.as_ref())
    }

    /// [`open_with`](Self::open_with) with no progress reports and no cancellation.
    pub fn open(&self, repo: &RepoRef, project: &Project, req: &OpenRequest) -> Result<Opened> {
        self.open_with(repo, project, req, &|_| {}, &Cancel::new())
    }

    /// Opens a target, reporting each [`Stage`] (and git's percentages) to `progress`. When
    /// `cancel` is set, the running git command and its children are stopped, the lock is
    /// released, and the cache is left consistent: a clone in progress is removed, refs are
    /// only those fully fetched, and a worktree being placed is removed and unregistered.
    pub fn open_with(
        &self,
        repo: &RepoRef,
        project: &Project,
        req: &OpenRequest,
        progress: &dyn Fn(Stage),
        cancel: &Cancel,
    ) -> Result<Opened> {
        self.git.check_version()?;
        let host = Self::host(repo)?;
        let paths = self.cache.repo(repo)?;
        let url = hosts::clone_url(repo, project)?;
        cho_core::git::check_remote_url(&url).map_err(Error::msg)?;
        progress(Stage::Lock);
        let _lock = self.cache.lock_with(&paths, self.lock_timeout, cancel)?;
        let mut notes = vec![];
        self.ensure_clone(repo, &paths, &url, &host, progress, cancel)?;
        let bare = paths.bare.as_path();
        let mut state = RepoState::load(&paths.state);

        // The remote's branches; the last fetched ones when it can't be reached.
        cancel.check()?;
        progress(Stage::ListRemote);
        let (heads, offline_reason) = match self.remote_heads(bare, &host, cancel) {
            Ok(h) => (h, None),
            Err(f) if f.cancelled || f.is_auth() => {
                return Err(self.auth_hint_for(&host, repo, &url, &f));
            }
            Err(f) => (self.local_heads(bare, &state), Some(f.to_string())),
        };
        let offline = offline_reason.is_some();
        if heads.default.is_some() {
            state.default_branch = heads.default.clone();
        }
        let default_branch = heads.default.clone().ok_or_else(|| {
            Error::msg(format!(
                "cannot tell the default branch of {repo}; the remote has no HEAD"
            ))
        })?;
        let need_branch = |b: &str| -> Result<()> {
            if heads.has(b) {
                Ok(())
            } else {
                Err(Error::not_found(format!(
                    "Branch `{b}` not found on {repo}{}",
                    if offline { " (in the last fetch)" } else { "" }
                )))
            }
        };

        // Resolve the head side and what it compares against.
        let mut path = req.path.clone();
        let mut pull_request = None;
        let mut rate_limit = None;
        let mut target_branch = default_branch.clone();
        let mut extra_specs: Vec<String> = vec![];
        let mut commit_wanted: Option<String> = None;
        let head = match &req.kind {
            TargetKind::Default => Head::Branch(default_branch.clone()),
            TargetKind::Branch(b) => {
                need_branch(b)?;
                Head::Branch(b.clone())
            }
            TargetKind::Commit(c) => {
                commit_wanted = Some(c.clone());
                Head::Commit(c.clone())
            }
            TargetKind::BranchOrCommit(h) => {
                if heads.has(h) {
                    // A commit of that name is checked after the fetch.
                    commit_wanted = Some(h.clone());
                    Head::Branch(h.clone())
                } else {
                    commit_wanted = Some(h.clone());
                    Head::Commit(h.clone())
                }
            }
            TargetKind::RefPath(rp) => match resolve_ref_path(rp, &heads.names())? {
                RefPathMatch::Branch { branch, path: p } => {
                    path = path.or(p);
                    Head::Branch(branch)
                }
                RefPathMatch::Commit { commit, path: p } => {
                    path = path.or(p);
                    commit_wanted = Some(commit.clone());
                    Head::Commit(commit)
                }
            },
            TargetKind::PullRequest(n) => {
                cancel.check()?;
                progress(Stage::PullRequest);
                let fresh = if offline {
                    Err(Error::new(ErrorKind::Offline, "the host can't be reached"))
                } else {
                    self.pull_request(repo, Some(project), *n)
                };
                cancel.check()?;
                // Fresh details are kept with the worktree; when they can't be read, the
                // saved ones stand in, marked stale.
                let saved = state
                    .worktrees
                    .get(&pr_slug(*n))
                    .and_then(|w| w.pull_request.clone());
                let snapshot = match fresh {
                    Ok((details, rate)) => {
                        rate_limit = rate;
                        Ok(PrSnapshot {
                            details,
                            fetched_at: now_unix(),
                            stale: false,
                        })
                    }
                    Err(e) => match saved {
                        Some(rec) => {
                            notes.push(format!(
                                "The pull request's details could not be refreshed ({e}); showing those read at {} (Unix time).",
                                rec.fetched_at
                            ));
                            Ok(PrSnapshot {
                                details: rec.details,
                                fetched_at: rec.fetched_at,
                                stale: true,
                            })
                        }
                        None => Err(e),
                    },
                };
                let local = format!("refs/chorong/pr/{n}");
                match snapshot {
                    Ok(snap) => {
                        let d = &snap.details;
                        if d.fork && matches!(host, Host::BitbucketCloud) {
                            return Err(Error::unsupported(
                                "Pull requests from forks aren't supported yet",
                            ));
                        }
                        if d.fork {
                            notes.push(
                                "This pull request comes from a fork; that is not tested.".into(),
                            );
                        }
                        if d.target_branch.is_empty() || !heads.has(&d.target_branch) {
                            notes.push(format!(
                                "The pull request's target `{}` is not a branch here; comparing with {default_branch}.",
                                d.target_branch
                            ));
                        } else {
                            target_branch = d.target_branch.clone();
                        }
                        let head = match host.pr_ref(*n) {
                            Some(remote) => Head::Pr {
                                number: *n,
                                local,
                                remote,
                            },
                            // No pull-request refs (Bitbucket Cloud): its source branch.
                            None => {
                                need_branch(&d.source_branch)?;
                                Head::Pr {
                                    number: *n,
                                    local: branch_ref(&d.source_branch),
                                    remote: branch_ref(&d.source_branch),
                                }
                            }
                        };
                        pull_request = Some(snap);
                        head
                    }
                    Err(e) => match host.pr_ref(*n) {
                        Some(remote) => {
                            notes.push(format!(
                                "The pull request's details could not be read ({e}); comparing with {default_branch}."
                            ));
                            Head::Pr {
                                number: *n,
                                remote,
                                local,
                            }
                        }
                        None => {
                            return Err(Error::new(
                                e.kind.clone(),
                                format!(
                                    "{e}\nBitbucket Cloud pull requests are opened from their details \
                                     (the source branch), which could not be read."
                                ),
                            ));
                        }
                    },
                }
            }
        };

        // The base override, if a branch, is fetched with the rest.
        enum Base {
            Auto,
            Branch(String),
            Commit(String),
        }
        let base = match req.base.as_deref().map(str::trim) {
            None | Some("") => Base::Auto,
            Some(b) if heads.has(b) => {
                // A commit of the same name is checked after the fetch.
                extra_specs.push(spec(&branch_ref(b), &branch_ref(b)));
                Base::Branch(b.to_string())
            }
            Some(b) if is_hex_commit(b) => Base::Commit(b.to_ascii_lowercase()),
            Some(b) => {
                return Err(Error::msg(format!(
                    "--base `{b}` is neither a branch of {repo} nor a commit id"
                )));
            }
        };

        // Fetch: the target branch, the head, and the base override.
        let mut specs = vec![spec(
            &branch_ref(&target_branch),
            &branch_ref(&target_branch),
        )];
        match &head {
            Head::Branch(b) => specs.push(spec(&branch_ref(b), &branch_ref(b))),
            Head::Pr { local, remote, .. } => specs.push(spec(remote, local)),
            Head::Commit(_) => {}
        }
        specs.extend(extra_specs);
        specs.sort();
        specs.dedup();
        // Every ref here came from the user, the remote or the host's API: check each is a
        // valid ref name before it reaches git.
        for s in &specs {
            for side in s.trim_start_matches('+').split(':') {
                check_ref(side)?;
            }
        }
        if !offline {
            cancel.check()?;
            progress(Stage::Fetch { progress: None });
            let mut args = vec![
                "fetch",
                "--no-tags",
                "--no-recurse-submodules",
                "--progress",
                "--update-head-ok",
                "--",
                "origin",
            ];
            args.extend(specs.iter().map(String::as_str));
            // A cancelled fetch updates no ref (git writes refs last), so the cache stays as
            // it was.
            let on_fetch = |p| progress(Stage::Fetch { progress: Some(p) });
            match self.network(Some(bare), &args, &host, &on_fetch, cancel) {
                Ok(_) => {}
                Err(f) => {
                    if let Head::Pr { number, .. } = &head
                        && !f.cancelled
                        && f.stderr.contains("couldn't find remote ref")
                    {
                        return Err(Error::not_found(format!(
                            "Pull request {number} not found on {repo}"
                        ))
                        .with_detail(f.to_string()));
                    }
                    return Err(self.auth_hint_for(&host, repo, &url, &f));
                }
            }
            let now = now_unix();
            for s in &specs {
                if let Some((_, local)) = s.split_once(':') {
                    state.fetched.insert(local.to_string(), now);
                }
            }
        } else if let Some(why) = &offline_reason {
            let when = state
                .fetched
                .get(&branch_ref(&target_branch))
                .map(|t| format!("the fetch at {t} (Unix time)"))
                .unwrap_or_else(|| "the last fetch".into());
            notes.push(format!("Could not reach the remote ({why}); using {when}."));
        }

        // Commits named by id: fetch a full id the fetch didn't bring.
        let fetch_commit = |c: &str| -> Option<String> {
            if let Some(full) = reads::commit_by_id(bare, c) {
                return Some(full);
            }
            if c.len() == 40 && !offline {
                let local = format!("refs/chorong/commits/{c}");
                let s = spec(c, &local);
                if self
                    .network(
                        Some(bare),
                        &[
                            "fetch",
                            "--no-tags",
                            "--no-recurse-submodules",
                            "--quiet",
                            "--",
                            "origin",
                            &s,
                        ],
                        &host,
                        &|_| {},
                        cancel,
                    )
                    .is_ok()
                {
                    return reads::commit_by_id(bare, c);
                }
            }
            None
        };
        let not_found = |c: &str| {
            if cancel.is_cancelled() {
                return Error::cancelled();
            }
            let e = Error::new(
                ErrorKind::CommitNotFound {
                    commit: c.to_string(),
                },
                format!("Commit `{c}` not found in {repo}"),
            );
            if c.len() < 40 {
                e.with_detail(
                    "An abbreviated commit id is found only in fetched branches; give the full id.",
                )
            } else {
                e
            }
        };

        let (head_id, opened_as, slug, is_default) = match &head {
            Head::Branch(b) => {
                if let Some(c) = &commit_wanted
                    && fetch_commit(c).is_some()
                {
                    return Err(ambiguous(c));
                }
                let id = reads::resolve_commit(bare, &branch_ref(b))
                    .ok_or_else(|| Error::not_found(format!("branch `{b}` was not fetched")))?;
                let default = *b == default_branch;
                (id, OpenedAs::Branch(b.clone()), branch_slug(b), default)
            }
            Head::Pr { number, local, .. } => {
                let id = reads::resolve_commit(bare, local).ok_or_else(|| {
                    Error::not_found(format!("pull request {number} was not fetched"))
                })?;
                if let Some(PrSnapshot { details: d, .. }) = &pull_request
                    && !d.head.is_empty()
                    && !id.starts_with(&d.head)
                {
                    notes.push(format!(
                        "The host reports head {} but the fetched head is {}; the pull request may have just changed.",
                        cho_git::short(&d.head),
                        cho_git::short(&id)
                    ));
                }
                (id, OpenedAs::PullRequest(*number), pr_slug(*number), false)
            }
            Head::Commit(c) => {
                let id = fetch_commit(c).ok_or_else(|| not_found(c))?;
                let slug = rev_slug(&id);
                (id.clone(), OpenedAs::Commit(id), slug, false)
            }
        };

        cancel.check()?;
        progress(Stage::MergeBase);
        let target_id = reads::resolve_commit(bare, &branch_ref(&target_branch))
            .ok_or_else(|| Error::not_found(format!("branch `{target_branch}` was not fetched")))?;
        let (before_id, base_source, merge_bases) = match base {
            Base::Auto => {
                let mb = cho_git::ObjectStore::open(bare)
                    .and_then(|s| s.merge_base(&head_id, &target_id))
                    .map_err(Error::msg)?
                    .ok_or_else(|| {
                        Error::msg(format!(
                            "{} and {target_branch} share no history; pass --base REV",
                            cho_git::short(&head_id)
                        ))
                    })?;
                let (first, bases) = (mb.chosen, mb.all);
                if bases.len() > 1 {
                    notes.push(format!(
                        "There are {} merge bases; Before is {} (the newest; ties go to the lowest id).",
                        bases.len(),
                        cho_git::short(&first)
                    ));
                }
                (first, BaseSource::MergeBase, bases.len())
            }
            Base::Branch(b) => {
                if is_hex_commit(&b) && fetch_commit(&b).is_some() {
                    return Err(ambiguous(&b));
                }
                let id = reads::resolve_commit(bare, &branch_ref(&b))
                    .ok_or_else(|| Error::msg(format!("branch `{b}` was not fetched")))?;
                (id, BaseSource::Override, 0)
            }
            Base::Commit(c) => {
                let id = fetch_commit(&c).ok_or_else(|| not_found(&c))?;
                (id, BaseSource::Override, 0)
            }
        };
        if head_id == target_id && base_source == BaseSource::MergeBase {
            notes.push(format!(
                "The head is {target_branch}'s tip, so Before and After are the same commit."
            ));
        }
        // The clone is blobless: fetch Before's contents of the changed files now, in one
        // batch (git diff prefetches them), so reading Before later needs no network.
        cancel.check()?;
        if before_id != head_id {
            progress(Stage::Prefetch { progress: None });
        }
        if before_id != head_id
            && let Err(f) = self.network(
                Some(bare),
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--ignore-submodules=all",
                    "--numstat",
                    &before_id,
                    &head_id,
                ],
                &host,
                &|p| progress(Stage::Prefetch { progress: Some(p) }),
                cancel,
            )
        {
            if f.cancelled {
                return Err(Error::cancelled());
            }
            notes.push(format!(
                "Before's copies of the changed files could not all be fetched ({f}); they are fetched when read."
            ));
        }
        let behind = reads::count_missing(bare, &head_id, &target_id)?;
        let ahead = reads::count_missing(bare, &target_id, &head_id)?;

        // Place the worktree, evicting the least recently opened beyond the cap.
        cancel.check()?;
        progress(Stage::Worktree { progress: None });
        let evicted = self.place_worktree(
            &paths,
            &mut state,
            &slug,
            &head_id,
            project.worktree_cap,
            &|p| progress(Stage::Worktree { progress: Some(p) }),
            cancel,
        )?;
        self.drop_indexes(&evicted);
        let pr_record = pull_request.as_ref().map(|p| PrRecord {
            details: p.details.clone(),
            fetched_at: p.fetched_at,
        });
        state.touch(
            &slug,
            WorktreeState {
                seq: 0,
                opened: now_unix(),
                head: head_id.clone(),
                what: opened_as.describe(),
                opened_as: Some(opened_as.clone()),
                default_branch: is_default,
                prepare: state.prepare_record(&slug),
                pull_request: pr_record.or_else(|| {
                    state
                        .worktrees
                        .get(&slug)
                        .and_then(|w| w.pull_request.clone())
                }),
            },
        );
        state.save(&paths.state)?;

        Ok(Opened {
            repo: repo.to_string(),
            worktree: paths.worktree(&slug),
            slug,
            opened_as,
            pull_request,
            head: reads::commit_info(bare, &head_id)?,
            before: reads::commit_info(bare, &before_id)?,
            base_source,
            merge_bases,
            target: reads::commit_info(bare, &target_id)?,
            target_fetched_at: state.fetched.get(&branch_ref(&target_branch)).copied(),
            target_branch,
            behind,
            ahead,
            offline,
            path,
            line: req.line,
            notes,
            evicted,
            rate_limit,
        })
    }

    /// Creates the worktree for `slug` at `head`, or moves an existing one there.
    #[allow(clippy::too_many_arguments)]
    fn place_worktree(
        &self,
        paths: &RepoPaths,
        state: &mut RepoState,
        slug: &str,
        head: &str,
        cap: usize,
        on: &dyn Fn(GitProgress),
        cancel: &Cancel,
    ) -> Result<Vec<PathBuf>> {
        let wt = paths.worktree(slug);
        self.cache.ensure_inside(&wt)?;
        let bare = paths.bare.as_path();
        let _ = self.git.run(Some(bare), &["worktree", "prune"]);
        // Forget worktrees whose folders are gone.
        state
            .worktrees
            .retain(|s, _| paths.worktree(s).join(".git").exists());
        if wt.join(".git").exists() {
            let current = self.git.run(Some(&wt), &["rev-parse", "HEAD"]).ok();
            if current.as_deref() != Some(head) {
                // Cancelled half way, the worktree still names its old commit (git moves
                // HEAD last), so the next open checks it out again.
                self.git
                    .run_watched(
                        Some(&wt),
                        &["checkout", "--progress", "--detach", "--force", head],
                        false,
                        on,
                        cancel,
                    )
                    .map_err(|f| f.to_error("git"))?;
            }
            return Ok(vec![]);
        }
        let mut evicted = vec![];
        for s in state.evictions(slug, cap) {
            evicted.push(self.remove_worktree_at(paths, &s)?);
            state.worktrees.remove(&s);
        }
        if wt.exists() {
            // A leftover folder that is not a worktree (an interrupted add).
            std::fs::remove_dir_all(&wt)
                .map_err(|e| Error::msg(format!("cannot remove {}: {e}", wt.display())))?;
        }
        std::fs::create_dir_all(&paths.worktrees)
            .map_err(|e| Error::msg(format!("cannot create {}: {e}", paths.worktrees.display())))?;
        let wt_s = wt.to_string_lossy().into_owned();
        if let Err(f) = self.git.run_watched(
            Some(bare),
            &["worktree", "add", "--detach", "--", &wt_s, head],
            false,
            on,
            cancel,
        ) {
            // Cancelled or failed: no half-registered worktree is left behind.
            let _ = std::fs::remove_dir_all(&wt);
            let _ = self.git.run(Some(bare), &["worktree", "prune"]);
            return Err(f.to_error("git"));
        }
        Ok(evicted)
    }

    fn remove_worktree_at(&self, paths: &RepoPaths, slug: &str) -> Result<PathBuf> {
        let wt = paths.worktree(slug);
        self.cache.ensure_inside(&wt)?;
        let canonical = wt.canonicalize().unwrap_or_else(|_| wt.clone());
        let wt_s = wt.to_string_lossy().into_owned();
        if self
            .git
            .run(
                Some(&paths.bare),
                &["worktree", "remove", "--force", "--force", &wt_s],
            )
            .is_err()
            && wt.exists()
        {
            std::fs::remove_dir_all(&wt)
                .map_err(|e| Error::msg(format!("cannot remove {}: {e}", wt.display())))?;
            let _ = self.git.run(Some(&paths.bare), &["worktree", "prune"]);
        }
        Ok(canonical)
    }

    /// The pull request's head on the host now. GitHub and Data Center: `git ls-remote` of
    /// its head ref (no REST quota, no token needed for public repositories). Bitbucket Cloud
    /// has no pull-request refs, so its REST details are read and their abbreviated head
    /// matched against fetched commits.
    pub fn pr_head(&self, repo: &RepoRef, project: &Project, number: u64) -> Result<PrHead> {
        let host = Self::host(repo)?;
        let paths = self.cache.repo(repo)?;
        let Some(refname) = host.pr_ref(number) else {
            let (d, _) = self.pull_request(repo, Some(project), number)?;
            if d.head.is_empty() {
                return Err(Error::not_found(format!(
                    "the host gave no head for pull request {number}"
                )));
            }
            let full = paths
                .bare
                .exists()
                .then(|| reads::commit_by_id(&paths.bare, &d.head))
                .flatten();
            return Ok(match full {
                Some(id) => PrHead {
                    id,
                    abbreviated: false,
                },
                None => PrHead {
                    abbreviated: d.head.len() < 40,
                    id: d.head,
                },
            });
        };
        check_ref(&refname)?;
        let url = hosts::clone_url(repo, project)?;
        cho_core::git::check_remote_url(&url).map_err(Error::msg)?;
        let dir = self.network_dir(&paths)?;
        let out = self
            .network(
                Some(&dir),
                &["ls-remote", "--", &url, &refname],
                &host,
                &|_| {},
                &Cancel::new(),
            )
            .map_err(|f| self.auth_hint_for(&host, repo, &url, &f))?;
        let id = out
            .lines()
            .find_map(|l| l.split_once('\t').map(|(id, _)| id.trim().to_string()))
            .filter(|id| id.len() == 40 && id.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| {
                Error::not_found(format!("Pull request {number} not found on {repo}"))
            })?;
        Ok(PrHead {
            id,
            abbreviated: false,
        })
    }

    /// Drops the semantic indexes of worktrees that were removed.
    pub fn drop_indexes(&self, worktrees: &[PathBuf]) {
        if let Some(ix) = &self.index_cache {
            for w in worktrees {
                ix.remove_repo(w);
            }
        }
    }

    /// Forgets a project: removes it from `projects`, and its clone, worktrees, state and
    /// their indexes from the cache. The CLI and the settings page both use this.
    pub fn remove_project(
        &self,
        projects: &mut crate::Projects,
        repo: &RepoRef,
    ) -> Result<RemovedProject> {
        let was_project = projects.remove(repo)?;
        let worktrees = crate::usage::clear_repo(&self.cache, repo)?;
        self.drop_indexes(&worktrees);
        Ok(RemovedProject {
            was_project,
            worktrees,
        })
    }

    /// Clears the cache: everything (`repo` None), one repository, or one of its worktrees
    /// (`slug`), with their indexes. Returns the worktree folders removed.
    pub fn clear(&self, repo: Option<&RepoRef>, slug: Option<&str>) -> Result<Vec<PathBuf>> {
        let removed = match (repo, slug) {
            // A worktree names a folder inside one repository: without the repository it
            // would otherwise fall through to clearing everything.
            (None, Some(s)) => {
                return Err(Error::msg(format!(
                    "worktree `{s}` needs its repository; nothing was removed"
                )));
            }
            (None, None) => crate::usage::clear_all(&self.cache)?,
            (Some(r), None) => crate::usage::clear_repo(&self.cache, r)?,
            (Some(r), Some(s)) => vec![self.remove_worktree(r, s)?],
        };
        self.drop_indexes(&removed);
        Ok(removed)
    }

    /// Removes one worktree of `repo` (by its folder name). Returns its canonical path as it
    /// was, so the caller can drop its index.
    pub fn remove_worktree(&self, repo: &RepoRef, slug: &str) -> Result<PathBuf> {
        let paths = self.cache.repo(repo)?;
        if slug.contains('/') || slug.starts_with('.') || !paths.worktree(slug).exists() {
            return Err(Error::not_found(format!(
                "{repo} has no worktree `{slug}` in the cache"
            )));
        }
        let _lock = self.cache.lock(&paths, self.lock_timeout)?;
        let removed = self.remove_worktree_at(&paths, slug)?;
        let mut state = RepoState::load(&paths.state);
        if state.worktrees.remove(slug).is_some() {
            state.save(&paths.state)?;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests;
