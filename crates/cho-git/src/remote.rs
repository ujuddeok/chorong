//! Remote URLs to repository references.
//!
//! Recognised: GitHub (`github.com`), Bitbucket Cloud (`bitbucket.org`), and Bitbucket Data
//! Center by its URL shapes (`https://HOST[/context]/scm/PROJECT/NAME.git`, or SSH on port
//! 7999). Anything else (GitLab, a plain file path, an unknown host) is not guessed at:
//! the caller falls back to a `local:` reference.

use cho_core::RepoRef;

/// The repository `url` names, in Chorong's link form, if Chorong recognises the host.
pub fn repo_ref_for_remote(url: &str) -> Option<RepoRef> {
    let (host, port, path) = split(url.trim())?;
    let host = host.to_ascii_lowercase();
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    match host.as_str() {
        "github.com" | "www.github.com" if parts.len() == 2 => Some(RepoRef::GitHub {
            owner: parts[0].into(),
            name: parts[1].into(),
        }),
        "bitbucket.org" | "www.bitbucket.org" if parts.len() == 2 => Some(RepoRef::Bitbucket {
            workspace: parts[0].into(),
            name: parts[1].into(),
        }),
        "github.com" | "www.github.com" | "bitbucket.org" | "www.bitbucket.org" => None,
        _ => {
            // Data Center over HTTP(S): `…/scm/PROJECT/NAME`.
            if let Some(i) = parts.iter().position(|p| *p == "scm")
                && parts.len() == i + 3
            {
                return Some(RepoRef::BitbucketDc {
                    host,
                    project: parts[i + 1].into(),
                    name: parts[i + 2].into(),
                });
            }
            // Data Center over SSH: port 7999, `PROJECT/NAME`.
            (port == Some(7999) && parts.len() == 2).then(|| RepoRef::BitbucketDc {
                host,
                project: parts[0].into(),
                name: parts[1].into(),
            })
        }
    }
}

/// `(host, port, path)` of a URL (`scheme://[user@]host[:port]/path`) or an scp-like address
/// (`[user@]host:path`). `None` for local paths.
fn split(url: &str) -> Option<(&str, Option<u16>, &str)> {
    if let Some((scheme, rest)) = url.split_once("://") {
        if !matches!(
            scheme,
            "https" | "http" | "ssh" | "git" | "git+ssh" | "ssh+git"
        ) {
            return None;
        }
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let (host, port) = match host_port.rsplit_once(':') {
            Some((h, p)) => (h, p.parse().ok()),
            None => (host_port, None),
        };
        return (!host.is_empty()).then_some((host, port, path));
    }
    // scp-like: no slash before the first colon, and not a Windows drive letter.
    let (head, path) = url.split_once(':')?;
    if head.contains('/') || head.len() < 2 {
        return None;
    }
    let host = head.rsplit_once('@').map_or(head, |(_, h)| h);
    (!host.is_empty()).then_some((host, None, path))
}
