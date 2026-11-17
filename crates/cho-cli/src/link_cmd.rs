//! `cho link parse`, `link build` and `web-url`: `chorong://` links and host web URLs.

use crate::{BuildLink, Output};
use anyhow::{Result, anyhow};
use cho_core::{ChorongLink, RepoRef, web_url};
use serde_json::json;

pub fn parse(url: String) -> Result<Output> {
    let link = ChorongLink::parse(&url)?;
    let json = match &link {
        ChorongLink::Open {
            repo,
            pr,
            rev,
            base,
            side,
            path,
            symbol,
            line,
            branch,
            dirty,
        } => json!({
            "action": "open", "repo": repo.to_string(), "rev": rev, "path": path, "symbol": symbol, "line": line,
            "branch": branch, "dirty": dirty,
            "pr": pr, "base": base, "side": side.map(|s| s.as_str()),
        }),
        ChorongLink::Investigation { src } => json!({ "action": "investigation", "src": src }),
    };
    Ok(Output::ok(serde_json::to_string_pretty(&json)?, json))
}

pub fn build(b: BuildLink) -> Result<Output> {
    let link = ChorongLink::Open {
        repo: RepoRef::parse(&b.repo)?,
        pr: None,
        rev: b.rev,
        base: None,
        side: None,
        path: b.path,
        symbol: b.symbol,
        line: b.line,
        branch: b.branch,
        dirty: b.dirty,
    };
    let url = link.to_url();
    ChorongLink::parse(&url)?; // validates path and line
    Ok(Output::ok(url.clone(), json!({ "url": url })))
}

pub fn web(repo: String, rev: String, path: String, line: Option<usize>) -> Result<Output> {
    let repo = RepoRef::parse(&repo)?;
    let url = web_url(&repo, &rev, &path, line)
        .ok_or_else(|| anyhow!("local repositories have no web URL"))?;
    Ok(Output::ok(url.clone(), json!({ "url": url })))
}
