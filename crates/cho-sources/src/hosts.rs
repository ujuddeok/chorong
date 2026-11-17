//! Hosts: clone URLs, and the one REST call Chorong makes, pull-request details (number,
//! title, author, state, source and target branch, head commit; nothing else). Blocking,
//! `ureq` with rustls, never on a UI thread.

use crate::auth::{Credential, Host};
use crate::projects::{Project, Protocol};
use crate::{Error, ErrorKind, Result};
use cho_core::RepoRef;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// The clone URL for `repo`: the project's explicit URL, else one built from the identity
/// and the project's protocol.
pub fn clone_url(repo: &RepoRef, project: &Project) -> Result<String> {
    if let Some(u) = &project.url {
        return Ok(u.clone());
    }
    let ssh = project.protocol == Protocol::Ssh;
    Ok(match repo {
        RepoRef::GitHub { owner, name } if ssh => format!("git@github.com:{owner}/{name}.git"),
        RepoRef::GitHub { owner, name } => format!("https://github.com/{owner}/{name}.git"),
        RepoRef::Bitbucket { workspace, name } if ssh => {
            format!("git@bitbucket.org:{workspace}/{name}.git")
        }
        RepoRef::Bitbucket { workspace, name } => {
            format!("https://bitbucket.org/{workspace}/{name}.git")
        }
        RepoRef::BitbucketDc {
            host,
            project,
            name,
        } if ssh => format!("ssh://git@{host}:7999/{project}/{name}.git"),
        RepoRef::BitbucketDc {
            host,
            project,
            name,
        } => format!("https://{host}/scm/{project}/{name}.git"),
        RepoRef::Local { .. } => {
            return Err(Error::msg(format!("`{repo}` is not a remote repository")));
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrState {
    Open,
    Merged,
    /// Closed without merging (GitHub "closed", Bitbucket "declined" or "superseded").
    Declined,
}

impl PrState {
    /// `open`, `merged` or `declined` (as in JSON).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Merged => "merged",
            Self::Declined => "declined",
        }
    }
}

/// What Chorong reads about a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrDetails {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub state: PrState,
    pub source_branch: String,
    pub target_branch: String,
    /// The head commit as the host reports it (Bitbucket Cloud abbreviates it).
    pub head: String,
    /// The source branch is in another repository (a fork).
    pub fork: bool,
}

/// GitHub's remaining REST quota, from the response headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RateLimit {
    pub remaining: u64,
    pub limit: u64,
}

/// Where the REST APIs are. `CHORONG_GITHUB_API_URL` and `CHORONG_BITBUCKET_API_URL` override the
/// public ones (tests, proxies).
#[derive(Debug, Clone)]
pub struct Api {
    pub github: String,
    pub bitbucket: String,
}

impl Default for Api {
    fn default() -> Self {
        Self {
            github: "https://api.github.com".into(),
            bitbucket: "https://api.bitbucket.org/2.0".into(),
        }
    }
}

impl Api {
    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let d = Self::default();
        Self {
            github: get("CHORONG_GITHUB_API_URL").unwrap_or(d.github),
            bitbucket: get("CHORONG_BITBUCKET_API_URL").unwrap_or(d.bitbucket),
        }
    }

    /// The REST base for a Data Center repository: its clone URL up to `/scm/`, else
    /// `https://HOST`.
    fn data_center(repo: &RepoRef, project: Option<&Project>) -> String {
        if let Some(u) = project.and_then(|p| p.url.as_deref())
            && let Some((base, _)) = u.split_once("/scm/")
            && base.starts_with("http")
        {
            return base.to_string();
        }
        match repo {
            RepoRef::BitbucketDc { host, .. } => format!("https://{host}"),
            _ => String::new(),
        }
    }
}

/// A response: status, body and GitHub's quota headers.
struct Response {
    status: u16,
    body: String,
    rate: Option<RateLimit>,
}

fn get(url: &str, auth: Option<String>) -> Result<Response> {
    let loopback = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .is_some_and(|h| h == "127.0.0.1" || h == "localhost" || h == "[::1]");
    let mut config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        // A token never follows a redirect, and never goes over plain HTTP (loopback is
        // for tests and local proxies).
        .redirect_auth_headers(ureq::config::RedirectAuthHeaders::Never)
        .https_only(!loopback)
        .user_agent(concat!("cho/", env!("CARGO_PKG_VERSION")))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .unversioned_rustls_crypto_provider(std::sync::Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .build(),
        );
    if loopback {
        config = config.proxy(None);
    }
    let agent: ureq::Agent = config.build().into();
    let mut req = agent
        .get(url)
        .header("Accept", "application/json, application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(a) = auth {
        req = req.header("Authorization", a);
    }
    let mut resp = req.call().map_err(|e| {
        let host = host_of(url);
        let detail = format!("GET {url}: {e}");
        let cause = match &e {
            ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
                Some("connection refused")
            }
            ureq::Error::HostNotFound => Some("host not found"),
            ureq::Error::Timeout(_) => Some("timed out"),
            ureq::Error::Io(_) | ureq::Error::ConnectionFailed => Some("no connection"),
            _ => None,
        };
        match cause {
            // A refused or unresolvable connection, or a timeout: no network to the host.
            Some(c) => Error::new(ErrorKind::Offline, format!("Can't reach {host} ({c})")),
            None => Error::msg(format!("Request to {host} failed: {e}")),
        }
        .with_detail(detail)
    })?;
    let header = |k: &str| {
        resp.headers()
            .get(k)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
    };
    let rate = header("x-ratelimit-remaining")
        .zip(header("x-ratelimit-limit"))
        .map(|(remaining, limit)| RateLimit { remaining, limit });
    let status = resp.status().as_u16();
    let body = resp
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| Error::msg(format!("reading the reply from {}: {e}", host_of(url))))?;
    Ok(Response { status, body, rate })
}

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "the host".into())
}

fn authorization(host: &Host, cred: Option<&Credential>) -> Result<Option<String>> {
    let Some(c) = cred else { return Ok(None) };
    Ok(Some(match host {
        Host::BitbucketCloud => {
            let email = c.email.as_deref().ok_or_else(|| {
                Error::msg(
                    "a Bitbucket Cloud token needs the account email: \
                     `cho auth add bitbucket.org --email EMAIL` or CHORONG_BITBUCKET_EMAIL",
                )
            })?;
            format!(
                "Basic {}",
                base64(format!("{email}:{}", c.token).as_bytes())
            )
        }
        _ => format!("Bearer {}", c.token),
    }))
}

fn status_error(host: &Host, what: &str, r: &Response, has_token: bool) -> Error {
    let quota_out = r.rate.is_some_and(|q| q.remaining == 0);
    let msg = match r.status {
        401 => {
            format!("{host} refused the token ({what}); check it with `cho auth status {host}`")
        }
        403 | 429 if quota_out && !has_token => format!(
            "GitHub's quota for unauthenticated requests is used up; add a token with \
             `cho auth add {host}`"
        ),
        403 | 429 if quota_out => format!("GitHub's quota for this token is used up ({what})"),
        403 => format!("{host} refused access to {what}"),
        404 if has_token => format!("{what} was not found (or the token can't see it)"),
        404 => format!(
            "{what} was not found; if the repository is private, add a token with \
             `cho auth add {host}`"
        ),
        s => format!("{host} answered {s} for {what}"),
    };
    let auth = ErrorKind::Auth {
        host: host.name().to_string(),
    };
    let kind = match r.status {
        401 | 403 | 429 => auth,
        // Private repositories answer 404 without a token.
        404 if !has_token => auth,
        404 => ErrorKind::NotFound,
        _ => ErrorKind::Other,
    };
    Error::new(kind, msg)
}

fn json(r: &Response, host: &Host) -> Result<Value> {
    serde_json::from_str(&r.body)
        .map_err(|e| Error::msg(format!("{host} sent a reply Chorong can't read: {e}")))
}

fn s(v: &Value, path: &[&str]) -> String {
    path.iter()
        .try_fold(v, |v, k| v.get(*k))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Pull request `number` of `repo`, and GitHub's remaining quota.
pub fn pull_request(
    api: &Api,
    repo: &RepoRef,
    project: Option<&Project>,
    number: u64,
    cred: Option<&Credential>,
) -> Result<(PrDetails, Option<RateLimit>)> {
    let host =
        Host::of(repo).ok_or_else(|| Error::msg(format!("`{repo}` is not a remote repository")))?;
    let what = format!("pull request {number} of {repo}");
    let url = match repo {
        RepoRef::GitHub { owner, name } => {
            format!("{}/repos/{owner}/{name}/pulls/{number}", api.github)
        }
        RepoRef::Bitbucket { workspace, name } => format!(
            "{}/repositories/{workspace}/{name}/pullrequests/{number}",
            api.bitbucket
        ),
        RepoRef::BitbucketDc {
            project: p, name, ..
        } => format!(
            "{}/rest/api/1.0/projects/{p}/repos/{name}/pull-requests/{number}",
            Api::data_center(repo, project)
        ),
        RepoRef::Local { .. } => unreachable!("Host::of"),
    };
    let r = get(&url, authorization(&host, cred)?).map_err(|mut e| {
        // The API is somewhere other than the host's own (an override, a proxy, a test
        // server): say which repository it was for.
        if e.kind == ErrorKind::Offline && host_of(&url) != host.api_host() {
            e.message.push_str(&format!(" for {repo}"));
        }
        e
    })?;
    if r.status != 200 {
        return Err(status_error(&host, &what, &r, cred.is_some()));
    }
    let v = json(&r, &host)?;
    let details = match host {
        Host::GitHub => github_pr(&v),
        Host::BitbucketCloud => bitbucket_pr(&v),
        Host::BitbucketDc(_) => data_center_pr(&v),
    }
    .ok_or_else(|| Error::msg(format!("{host}'s reply for {what} is missing fields")))?;
    Ok((details, r.rate))
}

fn github_pr(v: &Value) -> Option<PrDetails> {
    let merged = v.get("merged_at").is_some_and(|m| !m.is_null())
        || v.get("merged").and_then(Value::as_bool) == Some(true);
    let state = match (v.get("state")?.as_str()?, merged) {
        (_, true) => PrState::Merged,
        ("open", _) => PrState::Open,
        _ => PrState::Declined,
    };
    let head_repo = s(v, &["head", "repo", "full_name"]);
    let base_repo = s(v, &["base", "repo", "full_name"]);
    Some(PrDetails {
        number: v.get("number")?.as_u64()?,
        title: s(v, &["title"]),
        author: s(v, &["user", "login"]),
        state,
        source_branch: s(v, &["head", "ref"]),
        target_branch: s(v, &["base", "ref"]),
        head: v.get("head")?.get("sha")?.as_str()?.to_string(),
        // A deleted fork has no head repository; that is a fork too.
        fork: head_repo != base_repo,
    })
}

fn bitbucket_pr(v: &Value) -> Option<PrDetails> {
    let state = match v.get("state")?.as_str()? {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        _ => PrState::Declined,
    };
    let src = s(v, &["source", "repository", "full_name"]);
    let dst = s(v, &["destination", "repository", "full_name"]);
    Some(PrDetails {
        number: v.get("id")?.as_u64()?,
        title: s(v, &["title"]),
        author: s(v, &["author", "display_name"]),
        state,
        source_branch: v
            .get("source")?
            .get("branch")?
            .get("name")?
            .as_str()?
            .into(),
        target_branch: s(v, &["destination", "branch", "name"]),
        head: s(v, &["source", "commit", "hash"]),
        fork: !src.is_empty() && !dst.is_empty() && src != dst,
    })
}

fn data_center_pr(v: &Value) -> Option<PrDetails> {
    let state = match v.get("state")?.as_str()? {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        _ => PrState::Declined,
    };
    let repo_id = |side: &str| {
        v.get(side)
            .and_then(|r| r.get("repository"))
            .and_then(|r| r.get("id"))
            .and_then(Value::as_u64)
    };
    Some(PrDetails {
        number: v.get("id")?.as_u64()?,
        title: s(v, &["title"]),
        author: s(v, &["author", "user", "name"]),
        state,
        source_branch: v.get("fromRef")?.get("displayId")?.as_str()?.into(),
        target_branch: s(v, &["toRef", "displayId"]),
        head: s(v, &["fromRef", "latestCommit"]),
        fork: repo_id("fromRef") != repo_id("toRef"),
    })
}

/// Whether a token works, from a cheap authenticated call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TokenCheck {
    Valid,
    Invalid,
    /// Not checked (Data Center) or the host could not be reached.
    Unknown(String),
}

pub fn check_token(api: &Api, host: &Host, cred: &Credential) -> (TokenCheck, Option<RateLimit>) {
    let url = match host {
        Host::GitHub => format!("{}/user", api.github),
        Host::BitbucketCloud => format!("{}/user", api.bitbucket),
        Host::BitbucketDc(_) => {
            return (
                TokenCheck::Unknown("not checked for Data Center".into()),
                None,
            );
        }
    };
    let auth = match authorization(host, Some(cred)) {
        Ok(a) => a,
        Err(e) => return (TokenCheck::Unknown(e.to_string()), None),
    };
    match get(&url, auth) {
        Ok(r) if r.status == 200 => (TokenCheck::Valid, r.rate),
        Ok(r) if r.status == 401 || r.status == 403 => (TokenCheck::Invalid, r.rate),
        Ok(r) => (
            TokenCheck::Unknown(format!("{host} answered {}", r.status)),
            r.rate,
        ),
        Err(e) => (TokenCheck::Unknown(e.to_string()), None),
    }
}

/// Standard base64 with padding (for HTTP Basic).
fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
