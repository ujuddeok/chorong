//! Remote sources: `cho project`, `cho open REPO …`, `cho pr`, `cho auth`, `cho cache`,
//! and the `cho credential` helper git calls. See `docs/public/sources.md`.

use crate::Output;
use anyhow::{Result, anyhow, bail};
use cho_core::RepoRef;
use cho_core::words::size_text;
use cho_semantic::IndexCache;
use cho_sources::auth::{Keychain, process_env};
use cho_sources::hosts::{TokenCheck, check_token};
use cho_sources::{
    Api, Cache, Git, Host, OpenRequest, Opened, Project, Projects, Protocol, Sources, Target,
    TargetKind, Tokens, parse_repo, parse_target, target, usage,
};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::io::{IsTerminal, Read};
use std::path::Path;

#[derive(Subcommand)]
pub enum ProjectCmd {
    /// List remembered remote repositories.
    List,
    /// Remember a repository: `github:OWNER/NAME`, `bitbucket:WS/NAME`,
    /// `bitbucket-dc:HOST/PROJECT/NAME`, or any URL from it.
    Add {
        repo: String,
        /// Clone over `ssh` or `https` (default https).
        #[arg(long, value_parser = ["ssh", "https"])]
        protocol: Option<String>,
        /// Clone from this URL instead of the one built from the name (a mirror, a Data
        /// Center server under a context path).
        #[arg(long)]
        url: Option<String>,
        /// Worktrees to keep before the least recently opened is removed.
        #[arg(long)]
        worktree_cap: Option<usize>,
    },
    /// Forget a repository and remove its cache (clone, worktrees and their indexes).
    Remove { repo: String },
}

#[derive(Subcommand)]
pub enum AuthCmd {
    /// Store a token in the OS keychain. The token is read from standard input, never from
    /// the command line.
    Add {
        /// `github.com`, `bitbucket.org`, or a Bitbucket Data Center host.
        host: String,
        /// The Atlassian account email (Bitbucket Cloud).
        #[arg(long)]
        email: Option<String>,
    },
    /// Whether a host has a token, where it comes from and whether it works. Never prints
    /// the token.
    Status {
        /// One host; default: github.com, bitbucket.org and the Data Center hosts of your
        /// projects.
        host: Option<String>,
        /// Don't call the host to check the token.
        #[arg(long)]
        no_check: bool,
    },
    /// Remove a host's token from the keychain.
    Remove { host: String },
}

#[derive(Subcommand)]
pub enum CacheCmd {
    /// Disk use per repository and worktree.
    List,
    /// Remove everything, one repository, or one worktree (with their indexes).
    Clear {
        /// A repository (`github:OWNER/NAME` or a URL); default: every repository.
        project: Option<String>,
        /// A worktree folder name, as `cho cache list` shows it.
        worktree: Option<String>,
    },
}

/// `cho open REPO --branch X | --pr N | --rev SHA | URL [--base REV]`.
#[derive(Args, Clone, Default)]
pub struct OpenRemote {
    /// Open this branch.
    #[arg(long, group = "what")]
    pub branch: Option<String>,
    /// Open this pull request.
    #[arg(long, group = "what")]
    pub pr: Option<u64>,
    /// Open this commit.
    #[arg(long, group = "what")]
    pub rev: Option<String>,
    /// Before: this branch or commit instead of the merge base with the target branch.
    #[arg(long)]
    pub base: Option<String>,
    /// Then run the project's approved prepare recipe and index the worktree. Needs an
    /// approval (`cho prepare approve REPO`, or a yes when asked on a terminal).
    #[arg(long)]
    pub prepare: bool,
    /// Then open the worktree in the Chorong viewer (as `cho PATH` does). Without it, only
    /// the worktree, Before and After are printed (for scripts and agents).
    #[arg(long)]
    pub view: bool,
}

impl OpenRemote {
    /// A flag that only a remote open takes (`--view` is shared with a local `--compare`).
    pub fn picks_remote(&self) -> bool {
        self.branch.is_some()
            || self.pr.is_some()
            || self.rev.is_some()
            || self.base.is_some()
            || self.prepare
    }

    fn any(&self) -> bool {
        self.branch.is_some()
            || self.pr.is_some()
            || self.rev.is_some()
            || self.base.is_some()
            || self.prepare
            || self.view
    }
}

/// True when `cho open TARGET` should open a remote repository rather than a local file,
/// folder or `chorong://` link.
pub fn is_remote_open(target: &str, flags: &OpenRemote) -> bool {
    if flags.any() {
        return true;
    }
    if target.starts_with("chorong://") || Path::new(target).exists() {
        return false;
    }
    parse_repo(target).is_ok()
}

static KEYCHAIN: Keychain = Keychain;

fn tokens() -> Tokens<'static> {
    Tokens {
        store: &KEYCHAIN,
        env: &process_env,
    }
}

pub fn cache() -> Result<Cache> {
    Cache::default_location()
        .ok_or_else(|| anyhow!("cannot find a cache directory (HOME is not set)"))
}

pub fn sources() -> Result<Sources<'static>> {
    Ok(Sources {
        cache: cache()?,
        git: Git {
            credential_helper: std::env::current_exe().ok(),
        },
        tokens: tokens(),
        api: Api::from_env(),
        lock_timeout: cho_sources::LOCK_TIMEOUT,
        index_cache: IndexCache::default_location(),
    })
}

pub fn repo_arg(s: &str) -> Result<(RepoRef, Option<String>)> {
    parse_repo(s).map_err(|e| anyhow!("{e}"))
}

pub fn project(cmd: ProjectCmd) -> Result<Output> {
    let mut projects = Projects::open_default()?;
    match cmd {
        ProjectCmd::List => {
            let list = projects.list();
            let text = if list.is_empty() {
                "no projects; add one with `cho project add URL`".to_string()
            } else {
                list.iter()
                    .map(|(r, p)| project_line(r, p))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let json = list
                .iter()
                .map(|(r, p)| project_json(r, p))
                .collect::<Vec<_>>();
            Ok(Output::ok(text, json!({ "projects": json })))
        }
        ProjectCmd::Add {
            repo,
            protocol,
            url,
            worktree_cap,
        } => {
            let (r, url_from_input) = repo_arg(&repo)?;
            let mut p = projects.get(&r).cloned().unwrap_or_default();
            if let Some(proto) = protocol {
                p.protocol = Protocol::parse(&proto)?;
            }
            if let Some(u) = url.or(url_from_input) {
                p.url = Some(u);
            }
            if let Some(c) = worktree_cap {
                p.worktree_cap = c;
            }
            let new = projects.insert(&r, p.clone())?;
            Ok(Output::ok(
                format!(
                    "{} {}",
                    if new { "added" } else { "updated" },
                    project_line(&r, &p)
                ),
                json!({ "added": new, "project": project_json(&r, &p) }),
            ))
        }
        ProjectCmd::Remove { repo } => {
            let (r, _) = repo_arg(&repo)?;
            let out = sources()?.remove_project(&mut projects, &r)?;
            let (had, removed) = (out.was_project, out.worktrees);
            Ok(Output {
                text: if had {
                    format!("removed {r} and its cache")
                } else {
                    format!("{r} is not a project")
                },
                json: json!({ "removed": had, "worktrees_removed": removed.len() }),
                ok: had,
            })
        }
    }
}

fn project_line(r: &RepoRef, p: &Project) -> String {
    format!(
        "{r}  {}{}  cap {}",
        p.protocol.as_str(),
        p.url
            .as_deref()
            .map(|u| format!(" ({u})"))
            .unwrap_or_default(),
        p.worktree_cap
    )
}

fn project_json(r: &RepoRef, p: &Project) -> Value {
    json!({
        "repo": r.to_string(),
        "protocol": p.protocol.as_str(),
        "url": p.url,
        "worktree_cap": p.worktree_cap,
    })
}

/// Opens a remote target: fetches, places the worktree and reports Before and After.
pub fn open(target: &str, flags: &OpenRemote, dry_run: bool) -> Result<Output> {
    let (repo, url, t) = resolve(target, flags)?;
    let (opened, project) =
        open_target(&repo, url, t.clone(), flags.base.clone().or(t.base.clone()))?;
    let mut text = open_text(&opened);
    let mut json = serde_json::to_value(&opened)?;
    if let Some(p) = crate::prepare_cmd::after_open(&repo, &project, &opened, flags.prepare)? {
        text.push_str(&p.text);
        json["prepare"] = p.json;
    }
    if flags.view {
        let launched = crate::open_cmd::launch_or_print(
            cho_core::Launch::Command {
                program: crate::open_cmd::viewer_path().display().to_string(),
                args: vec![opened.worktree.display().to_string()],
            },
            dry_run,
        )?;
        if dry_run {
            text.push_str(&launched.text);
            text.push('\n');
        }
        json["viewer"] = launched.json;
    }
    Ok(Output::ok(text, json))
}

pub fn open_remote(target: &str, flags: &OpenRemote) -> Result<Opened> {
    let (repo, url, t) = resolve(target, flags)?;
    Ok(open_target(&repo, url, t.clone(), flags.base.clone().or(t.base.clone()))?.0)
}

/// The repository and target `cho open` names: REPO plus a flag, or a URL that names
/// everything.
pub fn resolve(target: &str, flags: &OpenRemote) -> Result<(RepoRef, Option<String>, Target)> {
    let r: (RepoRef, Option<String>, Target) =
        if flags.branch.is_some() || flags.pr.is_some() || flags.rev.is_some() {
            let (repo, url) = repo_arg(target)?;
            let kind = if let Some(b) = &flags.branch {
                target::check_branch_name(b)?;
                TargetKind::Branch(b.clone())
            } else if let Some(n) = flags.pr {
                TargetKind::PullRequest(n)
            } else {
                let r = flags.rev.clone().unwrap_or_default();
                if !target::is_hex_commit(&r) {
                    bail!("--rev expects a commit id (7 to 40 hexadecimal digits)");
                }
                TargetKind::Commit(r.to_ascii_lowercase())
            };
            let mut t = parse_target("")?;
            t.kind = kind;
            (repo, url, t)
        } else if target::is_repo_name(target) {
            // A repository name alone: its default branch.
            let (repo, url) = repo_arg(target)?;
            (repo, url, parse_target("")?)
        } else {
            let t = parse_target(target)?;
            let repo = t
                .repo
                .clone()
                .ok_or_else(|| anyhow!("`{target}` does not name a repository"))?;
            (repo, t.clone_url.clone(), t)
        };
    Ok(r)
}

/// Opens a resolved target (adding the project if needed); returns the project too.
pub fn open_target(
    repo: &RepoRef,
    url: Option<String>,
    t: Target,
    base: Option<String>,
) -> Result<(Opened, Project)> {
    let repo = repo.clone();
    let (project, added) = Projects::open_default()?.get_or_add(&repo, url)?;
    let s = sources()?;
    let mut opened = s.open(
        &repo,
        &project,
        &OpenRequest {
            kind: t.kind,
            path: t.path,
            line: t.line,
            base,
        },
    )?;
    if let Some(note) = added {
        opened.notes.insert(0, note);
    }
    // So `chorong://` links to this repository (and pull request) resolve to the worktree.
    let _ = cho_sources::recent::remember(&opened.worktree);
    Ok((opened, project))
}

/// How long ago `t` (seconds since the Unix epoch) was, from now.
pub fn ago(t: u64) -> String {
    cho_core::words::ago(cho_sources::now_unix(), t)
}

fn open_text(o: &Opened) -> String {
    let mut s = format!("{}\n", o.worktree.display());
    let what = match (&o.pull_request, &o.opened_as) {
        (Some(pr), _) => format!(
            "pull request {} \"{}\" ({}) by {}, {} → {}{}",
            pr.details.number,
            pr.details.title,
            pr.details.state.as_str(),
            pr.details.author,
            pr.details.source_branch,
            pr.details.target_branch,
            if pr.stale {
                format!(" (details from {}; offline)", ago(pr.fetched_at))
            } else {
                String::new()
            }
        ),
        (None, a) => a.describe(),
    };
    s.push_str(&format!("{} · {what}\n", o.repo));
    s.push_str(&format!("After   {}  {}\n", o.head.short, o.head.subject));
    s.push_str(&format!(
        "Before  {}  {}  ({})\n",
        o.before.short,
        o.before.subject,
        match o.base_source {
            cho_sources::BaseSource::MergeBase => format!("merge base with {}", o.target_branch),
            cho_sources::BaseSource::Override => "--base".to_string(),
        }
    ));
    s.push_str(&format!(
        "{} at {}, fetched {}; {} behind, {} ahead\n",
        o.target_branch,
        o.target.short,
        o.target_fetched_at
            .map(ago)
            .unwrap_or_else(|| "never".into()),
        o.behind,
        o.ahead
    ));
    if o.behind > 0 && o.pull_request.is_some() {
        s.push_str(&format!(
            "{} commit{} behind {}; must be rebased before merging\n",
            o.behind,
            if o.behind == 1 { "" } else { "s" },
            o.target_branch
        ));
    }
    if let Some(p) = &o.path {
        s.push_str(&format!(
            "file: {p}{}\n",
            o.line.map(|l| format!(":{l}")).unwrap_or_default()
        ));
    }
    for e in &o.evicted {
        s.push_str(&format!(
            "removed the least recently opened worktree {}\n",
            e.display()
        ));
    }
    if let Some(q) = o.rate_limit.filter(|q| q.remaining < 10) {
        s.push_str(&format!(
            "GitHub REST quota: {} of {} left; a token raises it (`cho auth add github.com`)\n",
            q.remaining, q.limit
        ));
    }
    for n in &o.notes {
        s.push_str(&format!("note: {n}\n"));
    }
    s
}

pub fn pr(repo: &str, number: u64) -> Result<Output> {
    let (r, url) = repo_arg(repo)?;
    let projects = Projects::open_default()?;
    let project = projects.get(&r).cloned().or_else(|| {
        url.map(|u| Project {
            url: Some(u),
            ..Project::default()
        })
    });
    let (d, rate) = sources()?.pull_request(&r, project.as_ref(), number)?;
    Ok(Output::ok(
        format!(
            "#{} {}\n{} · {} → {} · head {} · by {}{}",
            d.number,
            d.title,
            d.state.as_str(),
            d.source_branch,
            d.target_branch,
            cho_git::short(&d.head),
            d.author,
            if d.fork { " · from a fork" } else { "" }
        ),
        json!({ "repo": r.to_string(), "pull_request": d, "rate_limit": rate }),
    ))
}

fn read_token(host: &Host) -> Result<String> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        eprint!("Paste the token for {host} and press Enter (it is not shown): ");
        let echo = |on: bool| {
            let _ = std::process::Command::new("stty")
                .arg(if on { "echo" } else { "-echo" })
                .stdin(std::process::Stdio::inherit())
                .status();
        };
        echo(false);
        let mut line = String::new();
        let r = stdin.read_line(&mut line);
        echo(true);
        eprintln!();
        r?;
        Ok(line)
    } else {
        let mut s = String::new();
        stdin.lock().read_to_string(&mut s)?;
        Ok(s)
    }
}

pub fn auth(cmd: AuthCmd) -> Result<Output> {
    let t = tokens();
    match cmd {
        AuthCmd::Add { host, email } => {
            let h = Host::parse(&host)?;
            let token = read_token(&h)?;
            t.add(&h, &token, email.as_deref())?;
            Ok(Output::ok(
                format!("stored a token for {h} in the keychain"),
                json!({ "host": h.name(), "stored": true }),
            ))
        }
        AuthCmd::Remove { host } => {
            let h = Host::parse(&host)?;
            let removed = t.remove(&h)?;
            Ok(Output {
                text: if removed {
                    format!("removed the token for {h}")
                } else {
                    format!("no token for {h} in the keychain")
                },
                json: json!({ "host": h.name(), "removed": removed }),
                ok: removed,
            })
        }
        AuthCmd::Status { host, no_check } => {
            let hosts = match host {
                Some(h) => vec![Host::parse(&h)?],
                None => {
                    let mut v = vec![Host::GitHub, Host::BitbucketCloud];
                    if let Ok(p) = Projects::open_default() {
                        for (r, _) in p.list() {
                            if let Some(h) = Host::of(&r)
                                && !v.contains(&h)
                            {
                                v.push(h);
                            }
                        }
                    }
                    v
                }
            };
            let api = Api::from_env();
            let mut lines = vec![];
            let mut out = vec![];
            for h in hosts {
                let st = t.status(&h);
                let (check, rate) = match (no_check, t.get(&h)) {
                    (false, Some(c)) => {
                        let (k, r) = check_token(&api, &h, &c);
                        (Some(k), r)
                    }
                    _ => (None, None),
                };
                let mut line = format!(
                    "{}: {}",
                    h,
                    match st.source {
                        Some(s) => format!("token from {s}"),
                        None => format!(
                            "no token (add one with `cho auth add {h}` or set {})",
                            h.token_env()
                        ),
                    }
                );
                match &check {
                    Some(TokenCheck::Valid) => line.push_str(", valid"),
                    Some(TokenCheck::Invalid) => line.push_str(", REFUSED by the host"),
                    Some(TokenCheck::Unknown(why)) => {
                        line.push_str(&format!(", not checked ({why})"))
                    }
                    None => {}
                }
                if matches!(h, Host::BitbucketCloud) && st.source.is_some() && !st.has_email {
                    line.push_str("; no account email (needed for REST)");
                }
                if let Some(q) = rate {
                    line.push_str(&format!("; REST quota {} of {}", q.remaining, q.limit));
                }
                if let Some(e) = st.keychain_error.as_ref().filter(|_| st.source.is_none()) {
                    line.push_str(&format!("\n  keychain: {e}"));
                }
                lines.push(line);
                out.push(json!({
                    "host": h.name(),
                    "token": st.source.is_some(),
                    "source": st.source.map(|s| s.to_string()),
                    "email": st.has_email,
                    "check": check,
                    "rate_limit": rate,
                    "keychain_error": st.keychain_error,
                }));
            }
            Ok(Output::ok(lines.join("\n"), json!({ "hosts": out })))
        }
    }
}

/// `cho credential get|store|erase`: git's credential-helper protocol on stdin/stdout.
pub fn credential(operation: &str) -> Result<()> {
    let mut req = String::new();
    std::io::stdin().read_to_string(&mut req)?;
    let resp = cho_sources::auth::credential_response(operation, &req, &tokens());
    print!("{resp}");
    Ok(())
}

pub fn cache_cmd(cmd: CacheCmd) -> Result<Output> {
    let c = cache()?;
    match cmd {
        CacheCmd::List => {
            let list = usage::list(&c);
            let total: u64 = list.iter().map(|u| u.total_bytes).sum();
            let mut text = format!("Cache: {} in {}\n", size_text(total), c.root().display());
            for u in &list {
                text.push_str(&format!(
                    "{}  {} (clone {})\n",
                    u.repo,
                    size_text(u.total_bytes),
                    size_text(u.clone_bytes)
                ));
                for w in &u.worktrees {
                    text.push_str(&format!(
                        "  {}  {}{}\n",
                        w.slug,
                        size_text(w.bytes),
                        w.what
                            .as_deref()
                            .map(|x| format!("  {x}"))
                            .unwrap_or_default()
                    ));
                }
            }
            Ok(Output::ok(
                text,
                json!({ "root": c.root(), "total_bytes": total, "repos": list }),
            ))
        }
        CacheCmd::Clear { project, worktree } => {
            let repo = project
                .as_deref()
                .map(repo_arg)
                .transpose()?
                .map(|(r, _)| r);
            let removed = sources()?.clear(repo.as_ref(), worktree.as_deref())?;
            let what = match (&project, &worktree) {
                (None, _) => "the repository cache".to_string(),
                (Some(p), None) => format!("the cache of {p}"),
                (Some(p), Some(w)) => format!("worktree {w} of {p}"),
            };
            Ok(Output::ok(
                format!("cleared {what} ({} worktree(s) removed)", removed.len()),
                json!({ "cleared": what, "worktrees_removed": removed }),
            ))
        }
    }
}
