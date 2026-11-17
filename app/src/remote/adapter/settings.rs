//! The settings window's Projects, Cache and Accounts sections. All of it blocks (disk walks,
//! the keychain, REST): background executor only. Token values only ever pass through
//! `add_token` and `check_account` on their way to the keychain or the host; they are never
//! returned, logged or stored here.

use super::{KEYCHAIN, keychain_tokens, projects, real_sources, require_cache};
use cho_core::RepoRef;
use cho_sources::auth::SecretStore;
use cho_sources::{Api, Project, Projects, Sources};
use std::path::PathBuf;

/// What is known about a project's prepare approval without opening it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalFact {
    /// Approved at this commit (full id).
    Approved { commit: String },
    /// A recipe of the project's own is saved but not approved.
    NotApproved,
    /// The project has neither (a recipe in the repository itself is only seen on open).
    NoRecipe,
}

/// One project as the Projects section shows and edits it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    pub repo: String,
    pub protocol: cho_sources::Protocol,
    /// A clone URL that replaces the one built from the name (a mirror, a `file://` remote).
    pub url: Option<String>,
    pub cap: usize,
    pub approval: ApprovalFact,
}

fn project_info(repo: &RepoRef, p: &Project) -> ProjectInfo {
    ProjectInfo {
        repo: repo.to_string(),
        protocol: p.protocol,
        url: p.url.clone(),
        cap: p.worktree_cap,
        approval: match (&p.approval, &p.recipe) {
            (Some(a), _) => ApprovalFact::Approved {
                commit: a.commit.clone(),
            },
            (None, Some(_)) => ApprovalFact::NotApproved,
            (None, None) => ApprovalFact::NoRecipe,
        },
    }
}

/// Every remembered project, by name.
pub fn project_infos() -> Result<Vec<ProjectInfo>, String> {
    let projects = Projects::open_default().map_err(|e| e.to_string())?;
    Ok(project_infos_in(&projects))
}

fn project_infos_in(projects: &Projects) -> Vec<ProjectInfo> {
    projects
        .list()
        .into_iter()
        .map(|(r, p)| project_info(&r, p))
        .collect()
}

/// Saves a project's protocol and worktree cap through `Projects::insert` (atomic). The rest
/// of the entry is kept; a retired `auto_prepare` in an older file is dropped.
pub fn save_project(repo: &str, protocol: cho_sources::Protocol, cap: usize) -> Result<(), String> {
    let mut projects = Projects::open_default().map_err(|e| e.to_string())?;
    save_project_in(&mut projects, repo, protocol, cap)
}

fn save_project_in(
    projects: &mut Projects,
    repo: &str,
    protocol: cho_sources::Protocol,
    cap: usize,
) -> Result<(), String> {
    let repo = RepoRef::parse(repo).map_err(|e| e.to_string())?;
    let mut project = projects
        .get(&repo)
        .cloned()
        .ok_or_else(|| format!("{repo} is not a project any more"))?;
    project.protocol = protocol;
    project.worktree_cap = cap;
    projects
        .insert(&repo, project)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Forgets a project and deletes its cached clone, worktrees and their indexes. Returns how
/// many worktrees went. The user's own clones are never touched (the backend only works
/// inside the cache).
pub fn remove_project(repo: &str) -> Result<usize, String> {
    let mut projects = Projects::open_default().map_err(|e| e.to_string())?;
    let sources = real_sources(require_cache()?, None);
    remove_project_in(&sources, &mut projects, repo)
}

fn remove_project_in(
    sources: &Sources<'_>,
    projects: &mut Projects,
    repo: &str,
) -> Result<usize, String> {
    let repo = RepoRef::parse(repo).map_err(|e| e.to_string())?;
    sources
        .remove_project(projects, &repo)
        .map(|r| r.worktrees.len())
        .map_err(|e| e.message)
}

/// The cache folder and every repository in it with sizes.
#[derive(Debug, Clone)]
pub struct CacheUsage {
    pub root: PathBuf,
    pub repos: Vec<cho_sources::usage::RepoUsage>,
}

/// Measures the cache (walks every file in it: can take seconds on a big cache).
pub fn cache_usage() -> Result<CacheUsage, String> {
    let cache = require_cache()?;
    Ok(CacheUsage {
        repos: cho_sources::usage::list(&cache),
        root: cache.root().to_path_buf(),
    })
}

/// The size of one repository's cache (for the "Remove project" confirmation).
pub fn repo_size(repo: &str) -> Result<cho_sources::usage::RepoUsage, String> {
    let cache = require_cache()?;
    let repo = RepoRef::parse(repo).map_err(|e| e.to_string())?;
    cho_sources::usage::repo_usage(&cache, &repo).map_err(|e| e.message)
}

/// Clears the cache: everything, one repository (`cho cache clear REPO`) or one of its
/// worktrees, with their indexes. Returns how many worktrees went.
pub fn clear_cache(repo: Option<&str>, slug: Option<&str>) -> Result<usize, String> {
    let sources = real_sources(require_cache()?, None);
    clear_in(&sources, repo, slug)
}

fn clear_in(
    sources: &Sources<'_>,
    repo: Option<&str>,
    slug: Option<&str>,
) -> Result<usize, String> {
    let repo = repo
        .map(RepoRef::parse)
        .transpose()
        .map_err(|e| e.to_string())?;
    sources
        .clear(repo.as_ref(), slug)
        .map(|v| v.len())
        .map_err(|e| e.message)
}

/// The hosts the Accounts section lists: GitHub, Bitbucket Cloud, then each Data Center host
/// of the projects, in order of first appearance (as `cho auth status` does).
pub fn account_hosts(repos: &[RepoRef]) -> Vec<cho_sources::Host> {
    use cho_sources::Host;
    let mut v = vec![Host::GitHub, Host::BitbucketCloud];
    for r in repos {
        if let Some(h) = Host::of(r)
            && !v.contains(&h)
        {
            v.push(h);
        }
    }
    v
}

/// What the keychain and environment say about one host (never the token).
#[derive(Debug, Clone)]
pub struct AccountInfo {
    pub host: cho_sources::Host,
    pub status: cho_sources::auth::AuthStatus,
}

#[derive(Debug, Clone)]
pub struct Accounts {
    pub hosts: Vec<AccountInfo>,
    /// `Err` with a sentence when there is no keychain to store a token in.
    pub keychain: Result<(), String>,
}

/// Reads token status for every listed host. Blocks on the keychain.
pub fn accounts() -> Accounts {
    let repos = projects();
    let tokens = keychain_tokens();
    Accounts {
        hosts: account_hosts(&repos)
            .into_iter()
            .map(|host| AccountInfo {
                status: tokens.status(&host),
                host,
            })
            .collect(),
        keychain: KEYCHAIN.available(),
    }
}

/// How a "Check" ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub check: cho_sources::hosts::TokenCheck,
    pub rate: Option<cho_sources::RateLimit>,
}

/// Asks the host whether its token works (one authenticated request; GitHub and Bitbucket
/// Cloud only). Blocks on the network.
pub fn check_account(host: &str) -> Result<Checked, String> {
    let host = cho_sources::Host::parse(host).map_err(|e| e.to_string())?;
    let tokens = keychain_tokens();
    let cred = tokens
        .get(&host)
        .ok_or_else(|| format!("no token for {host}"))?;
    let (check, rate) = cho_sources::hosts::check_token(&Api::from_env(), &host, &cred);
    Ok(Checked { check, rate })
}

/// Stores a token (and, for Bitbucket Cloud, the account email) in the keychain.
pub fn add_token(host: &str, token: &str, email: Option<&str>) -> Result<(), String> {
    let host = cho_sources::Host::parse(host).map_err(|e| e.to_string())?;
    let tokens = keychain_tokens();
    tokens
        .add(&host, token, email.map(str::trim).filter(|e| !e.is_empty()))
        .map_err(|e| e.to_string())
}

/// Removes the host's keychain entries. `Ok(false)` when there were none.
pub fn remove_token(host: &str) -> Result<bool, String> {
    let host = cho_sources::Host::parse(host).map_err(|e| e.to_string())?;
    let tokens = keychain_tokens();
    tokens.remove(&host).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use std::path::Path;

    /// A project with a fake clone and two fake worktrees in a temporary cache.
    fn seeded(dir: &Path) -> (Sources<'static>, Projects, RepoRef) {
        let sources = sources_in(&dir.join("repos"));
        let repo = RepoRef::parse("github:acme/api").unwrap();
        let paths = sources.cache.repo(&repo).unwrap();
        for (d, f) in [
            (paths.bare.clone(), "pack"),
            (paths.worktree("pr-12"), "a.rs"),
            (paths.worktree("main"), "b.rs"),
        ] {
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(f), vec![0u8; 2048]).unwrap();
        }
        let mut projects = Projects::open(dir.join("projects.json")).unwrap();
        projects.insert(&repo, Project::default()).unwrap();
        (sources, projects, repo)
    }

    #[test]
    fn remove_project_deletes_the_entry_and_the_cache_only() {
        let dir = tempfile::tempdir().unwrap();
        let mine = dir.path().join("my-own-clone");
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::write(mine.join("keep.txt"), "mine").unwrap();
        let (sources, mut projects, repo) = seeded(dir.path());
        let paths = sources.cache.repo(&repo).unwrap();
        assert_eq!(cho_sources::usage::list(&sources.cache).len(), 1);
        let n = remove_project_in(&sources, &mut projects, "github:acme/api").unwrap();
        assert_eq!(n, 2);
        assert!(projects.get(&repo).is_none());
        assert!(!paths.bare.exists() && !paths.dir.exists());
        assert!(mine.join("keep.txt").exists());
    }

    #[test]
    fn clear_removes_one_worktree_one_project_or_everything() {
        let dir = tempfile::tempdir().unwrap();
        let (sources, projects, repo) = seeded(dir.path());
        let paths = sources.cache.repo(&repo).unwrap();
        assert_eq!(
            clear_in(&sources, Some("github:acme/api"), Some("pr-12")),
            Ok(1)
        );
        assert!(!paths.worktree("pr-12").exists() && paths.worktree("main").exists());
        assert!(clear_in(&sources, Some("github:acme/api"), Some("../x")).is_err());
        assert_eq!(clear_in(&sources, Some("github:acme/api"), None), Ok(1));
        assert!(!paths.bare.exists());
        // The project entry stays: clearing the cache is not forgetting the project.
        assert!(projects.get(&repo).is_some());
        let (sources, _p, repo) = seeded(&dir.path().join("again"));
        assert_eq!(clear_in(&sources, None, None), Ok(2));
        assert!(!sources.cache.repo(&repo).unwrap().bare.exists());
    }

    #[test]
    fn saving_a_project_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let (_s, mut projects, repo) = seeded(dir.path());
        let ssh = cho_sources::Protocol::Ssh;
        save_project_in(&mut projects, "github:acme/api", ssh, 3).unwrap();
        let reopened = Projects::open(dir.path().join("projects.json")).unwrap();
        let p = reopened.get(&repo).unwrap();
        assert_eq!((p.protocol, p.worktree_cap), (ssh, 3));
        assert!(save_project_in(&mut projects, "github:acme/api", ssh, 0).is_err());
        assert!(save_project_in(&mut projects, "github:other/x", ssh, 3).is_err());
        assert_eq!(
            project_infos_in(&projects)[0].approval,
            ApprovalFact::NoRecipe
        );
    }

    #[test]
    fn accounts_list_github_bitbucket_then_data_center_hosts() {
        let repos: Vec<RepoRef> = [
            "github:acme/api",
            "bitbucket-dc:git.example.com/PROJ/app",
            "bitbucket-dc:git.example.com/PROJ/other",
            "bitbucket-dc:code.corp.io/X/y",
            "bitbucket:ws/r",
        ]
        .iter()
        .map(|s| RepoRef::parse(s).unwrap())
        .collect();
        let names: Vec<String> = account_hosts(&repos)
            .iter()
            .map(|h| h.name().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "github.com",
                "bitbucket.org",
                "git.example.com",
                "code.corp.io"
            ]
        );
        assert_eq!(account_hosts(&[]).len(), 2);
    }
}
