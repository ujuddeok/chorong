//! Parsing and building `chorong://` links. The format is specified in
//! `docs/public/url-scheme.md`; keep the two in step.

use std::fmt;
use thiserror::Error;
use url::Url;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LinkError {
    #[error("not a valid URL: {0}")]
    Url(String),
    #[error("expected the chorong:// scheme, found {0}://")]
    Scheme(String),
    #[error("unknown action `{0}` (expected `open` or `investigation`)")]
    Action(String),
    #[error("missing required parameter `{0}`")]
    Missing(&'static str),
    #[error("invalid value for `{param}`: {reason}")]
    Invalid { param: &'static str, reason: String },
}

/// Where a repository lives.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RepoRef {
    /// `github:owner/name`
    GitHub { owner: String, name: String },
    /// `bitbucket:workspace/name` (Bitbucket Cloud)
    Bitbucket { workspace: String, name: String },
    /// `bitbucket-dc:host/PROJECT/name` (Bitbucket Data Center / Server)
    BitbucketDc {
        host: String,
        project: String,
        name: String,
    },
    /// `local:/absolute/path`, or `local:~/path` under the home directory (what Chorong
    /// generates, so links never carry the absolute path).
    Local { path: String },
}

impl RepoRef {
    pub fn parse(s: &str) -> Result<Self, LinkError> {
        let invalid = |reason: &str| LinkError::Invalid {
            param: "repo",
            reason: reason.into(),
        };
        let (kind, rest) = s
            .split_once(':')
            .ok_or_else(|| invalid("expected `<host>:<path>`"))?;
        let parts: Vec<&str> = rest.split('/').collect();
        let nonempty = |n: usize| parts.len() == n && parts.iter().all(|p| !p.is_empty());
        match kind {
            "github" if nonempty(2) => Ok(Self::GitHub {
                owner: parts[0].into(),
                name: parts[1].into(),
            }),
            "bitbucket" if nonempty(2) => Ok(Self::Bitbucket {
                workspace: parts[0].into(),
                name: parts[1].into(),
            }),
            "bitbucket-dc" if nonempty(3) => Ok(Self::BitbucketDc {
                host: parts[0].into(),
                project: parts[1].into(),
                name: parts[2].into(),
            }),
            "local"
                if (rest.starts_with('/') || rest == "~" || rest.starts_with("~/"))
                    && !rest.split('/').any(|c| c == "..") =>
            {
                Ok(Self::Local { path: rest.into() })
            }
            "github" | "bitbucket" | "bitbucket-dc" => Err(invalid("wrong number of path parts")),
            "local" => Err(invalid(
                "local paths must be absolute (or start with `~/`) and must not contain `..`",
            )),
            other => Err(invalid(&format!("unknown host kind `{other}`"))),
        }
    }
}

impl RepoRef {
    /// The folder a `local:` reference names, with a leading `~` expanded from `HOME`.
    /// `None` for remote references, or `~` without a home directory.
    pub fn local_path(&self) -> Option<std::path::PathBuf> {
        let Self::Local { path } = self else {
            return None;
        };
        let home = || {
            std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .map(std::path::PathBuf::from)
        };
        if path == "~" {
            home()
        } else if let Some(rest) = path.strip_prefix("~/") {
            Some(home()?.join(rest))
        } else {
            Some(path.into())
        }
    }
}

impl RepoRef {
    /// True for `local:/absolute/path`: a repository outside the home folder with no remote.
    /// Such a name means nothing on another machine.
    pub fn is_absolute_local(&self) -> bool {
        matches!(self, Self::Local { path } if path.starts_with('/'))
    }

    /// Refuses a repository that must not leave the machine (see "Export for sharing" in
    /// `docs/public/url-scheme.md`): `local:/absolute/path`. `local:~/…` and remotes pass.
    pub fn check_shareable(&self) -> Result<(), String> {
        if self.is_absolute_local() {
            return Err(format!(
                "cannot share `{self}`: it is outside your home folder and has no remote, so \
                 the link would carry an absolute path. Add a remote (`git remote add origin \
                 URL`) or name the repository with `--repo` (for example `--repo github:OWNER/NAME`)"
            ));
        }
        Ok(())
    }
}

impl fmt::Display for RepoRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitHub { owner, name } => write!(f, "github:{owner}/{name}"),
            Self::Bitbucket { workspace, name } => write!(f, "bitbucket:{workspace}/{name}"),
            Self::BitbucketDc {
                host,
                project,
                name,
            } => {
                write!(f, "bitbucket-dc:{host}/{project}/{name}")
            }
            Self::Local { path } => write!(f, "local:{path}"),
        }
    }
}

/// Which version of a file a link was made on (`side=`). Provisional, with `pr` and `base`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkSide {
    Before,
    After,
}

impl LinkSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
        }
    }
}

/// A parsed `chorong://` link.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // one short-lived value per link; boxing would change every pattern
pub enum ChorongLink {
    /// `chorong://open?repo=…&pr=…&rev=…&base=…&side=…&path=…&symbol=…&line=…`
    Open {
        repo: RepoRef,
        /// Pull request number (`pr`, provisional): open its pull-request view.
        pr: Option<u64>,
        /// Commit to open (a pull request's head with `pr`). `None` means the default branch
        /// / working tree.
        rev: Option<String>,
        /// The Before commit the link was made with (`base`, provisional): a full commit id.
        base: Option<String>,
        /// The side being read (`side`, provisional). Absent means After.
        side: Option<LinkSide>,
        /// Repository-relative path.
        path: Option<String>,
        /// Declaration to reveal (resolved by Chorong, never guessed).
        symbol: Option<String>,
        /// One-based line, used only when no symbol is given or it fails to resolve.
        line: Option<usize>,
        /// Branch checked out when the link was made (informational).
        branch: Option<String>,
        /// The working tree had uncommitted changes when the link was made.
        dirty: bool,
    },
    /// `chorong://investigation?src=…` where `src` is an https URL or an absolute file path.
    Investigation { src: String },
}

impl ChorongLink {
    pub fn parse(s: &str) -> Result<Self, LinkError> {
        let url = Url::parse(s).map_err(|e| LinkError::Url(e.to_string()))?;
        if url.scheme() != "chorong" {
            return Err(LinkError::Scheme(url.scheme().into()));
        }
        let get = |key: &str| {
            url.query_pairs()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.into_owned())
        };
        match url.host_str().unwrap_or_default() {
            "open" => {
                let repo = RepoRef::parse(&get("repo").ok_or(LinkError::Missing("repo"))?)?;
                let path = get("path").map(validate_path).transpose()?;
                let line = get("line")
                    .map(|v| match v.parse::<usize>() {
                        Ok(n) if n >= 1 => Ok(n),
                        _ => Err(LinkError::Invalid {
                            param: "line",
                            reason: "must be a positive integer".into(),
                        }),
                    })
                    .transpose()?;
                let pr = get("pr")
                    .map(|v| match v.parse::<u64>() {
                        Ok(n) if n >= 1 && v.bytes().all(|b| b.is_ascii_digit()) => Ok(n),
                        _ => Err(LinkError::Invalid {
                            param: "pr",
                            reason: "must be a positive integer".into(),
                        }),
                    })
                    .transpose()?;
                let base = get("base")
                    .map(|v| {
                        if matches!(v.len(), 40 | 64) && v.bytes().all(|b| b.is_ascii_hexdigit()) {
                            Ok(v.to_ascii_lowercase())
                        } else {
                            Err(LinkError::Invalid {
                                param: "base",
                                reason: "must be a full commit id (40 or 64 hex digits)".into(),
                            })
                        }
                    })
                    .transpose()?;
                let side = get("side")
                    .map(|v| match v.as_str() {
                        "before" => Ok(LinkSide::Before),
                        "after" => Ok(LinkSide::After),
                        _ => Err(LinkError::Invalid {
                            param: "side",
                            reason: "must be `before` or `after`".into(),
                        }),
                    })
                    .transpose()?;
                let rev = get("rev").map(|v| validate_ref("rev", v)).transpose()?;
                let branch = get("branch")
                    .map(|v| validate_ref("branch", v))
                    .transpose()?;
                Ok(Self::Open {
                    repo,
                    pr,
                    base,
                    side,
                    rev,
                    path,
                    symbol: get("symbol"),
                    line,
                    branch,
                    dirty: match get("dirty").as_deref() {
                        None | Some("0") => false,
                        Some("1") => true,
                        Some(_) => {
                            return Err(LinkError::Invalid {
                                param: "dirty",
                                reason: "must be `1` when present".into(),
                            });
                        }
                    },
                })
            }
            "investigation" => Ok(Self::Investigation {
                src: get("src").ok_or(LinkError::Missing("src"))?,
            }),
            other => Err(LinkError::Action(other.into())),
        }
    }

    /// Builds the canonical URL string for this link. `:` and `/` stay
    /// literal so links remain readable; everything else unsafe is escaped.
    pub fn to_url(&self) -> String {
        let mut params: Vec<(&str, String)> = Vec::new();
        let action = match self {
            Self::Open {
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
            } => {
                params.push(("repo", repo.to_string()));
                params.extend(pr.map(|v| ("pr", v.to_string())));
                params.extend(rev.clone().map(|v| ("rev", v)));
                params.extend(base.clone().map(|v| ("base", v)));
                params.extend(side.map(|v| ("side", v.as_str().to_string())));
                params.extend(path.clone().map(|v| ("path", v)));
                params.extend(symbol.clone().map(|v| ("symbol", v)));
                params.extend(line.map(|v| ("line", v.to_string())));
                params.extend(branch.clone().map(|v| ("branch", v)));
                if *dirty {
                    params.push(("dirty", "1".into()));
                }
                "open"
            }
            Self::Investigation { src } => {
                params.push(("src", src.clone()));
                "investigation"
            }
        };
        let query: Vec<String> = params
            .iter()
            .map(|(k, v)| format!("{k}={}", encode_query_value(v)))
            .collect();
        format!("chorong://{action}?{}", query.join("&"))
    }
}

/// Percent-encodes a query value, keeping characters that are safe and
/// readable inside a query (RFC 3986 `pchar` minus `&`, `=`, `+`).
fn encode_query_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        let keep = b.is_ascii_alphanumeric() || b"-._~:/@!$'()*,;".contains(&b);
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `rev` and `branch` come from untrusted links and reach git and a copyable shell command:
/// the branch-name rules of `git check-ref-format --branch` (no leading `-`, no spaces,
/// control characters or `~^:?*[\`, no `..`, `//` or `@{`, no `.`-leading components, no
/// trailing `/`, `.` or `.lock`). Commit ids pass.
fn validate_ref(param: &'static str, v: String) -> Result<String, LinkError> {
    let bad = v.is_empty()
        || v.starts_with('-')
        || v.starts_with('/')
        || v.ends_with('/')
        || v.ends_with('.')
        || v.ends_with(".lock")
        || v.contains("..")
        || v.contains("//")
        || v.contains("@{")
        || v == "@"
        || v.split('/').any(|c| c.starts_with('.'))
        || v.chars()
            .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c));
    if bad {
        return Err(LinkError::Invalid {
            param,
            reason: "must be a commit id or a valid git branch name".into(),
        });
    }
    Ok(v)
}

fn validate_path(path: String) -> Result<String, LinkError> {
    if path.starts_with('/') || path.split('/').any(|c| c == ".." || c.is_empty()) {
        return Err(LinkError::Invalid {
            param: "path",
            reason: "must be repository-relative with no empty or `..` components".into(),
        });
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_open_link() {
        let link = ChorongLink::parse(
            "chorong://open?repo=github:acme/api&rev=3f9c1e2&path=src/http/webhook.rs&symbol=WebhookHandler%3A%3Ahandle",
        )
        .unwrap();
        assert_eq!(
            link,
            ChorongLink::Open {
                repo: RepoRef::GitHub {
                    owner: "acme".into(),
                    name: "api".into()
                },
                pr: None,
                base: None,
                side: None,
                rev: Some("3f9c1e2".into()),
                path: Some("src/http/webhook.rs".into()),
                symbol: Some("WebhookHandler::handle".into()),
                line: None,
                branch: None,
                dirty: false,
            }
        );
    }

    #[test]
    fn round_trips() {
        let links = [
            ChorongLink::Open {
                repo: RepoRef::BitbucketDc {
                    host: "git.example.com".into(),
                    project: "PLAT".into(),
                    name: "core".into(),
                },
                pr: None,
                base: None,
                side: None,
                rev: None,
                path: Some("lib/a b.py".into()),
                symbol: None,
                line: Some(12),
                branch: None,
                dirty: false,
            },
            ChorongLink::Open {
                repo: RepoRef::Local {
                    path: "/home/me/src/chorong".into(),
                },
                pr: None,
                base: None,
                side: None,
                rev: None,
                path: None,
                symbol: None,
                line: None,
                branch: Some("feature/a&b#1".into()),
                dirty: true,
            },
            ChorongLink::Investigation {
                src: "https://example.com/inv.json?x=1&y=2".into(),
            },
        ];
        for link in links {
            assert_eq!(ChorongLink::parse(&link.to_url()).unwrap(), link);
        }
    }

    #[test]
    fn sharing_refuses_absolute_local_paths_only() {
        let ok = |s: &str| RepoRef::parse(s).unwrap().check_shareable();
        assert!(ok("github:acme/api").is_ok());
        assert!(ok("bitbucket:ws/api").is_ok());
        assert!(ok("local:~/code/chorong").is_ok());
        assert!(ok("local:~").is_ok());
        let err = ok("local:/srv/chorong").unwrap_err();
        assert!(
            err.contains("add a remote") || err.contains("Add a remote"),
            "{err}"
        );
        assert!(err.contains("--repo"), "{err}");
    }

    #[test]
    fn local_links_may_name_the_home_directory() {
        let url = "chorong://open?repo=local:~/code/chorong&path=src/main.rs";
        let ChorongLink::Open { repo, .. } = ChorongLink::parse(url).unwrap() else {
            panic!("expected an open link");
        };
        assert_eq!(repo.to_string(), "local:~/code/chorong");
        assert_eq!(ChorongLink::parse(url).unwrap().to_url(), url);
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            assert_eq!(
                repo.local_path(),
                Some(std::path::PathBuf::from(home).join("code/chorong"))
            );
        }
        assert!(RepoRef::parse("local:~/../x").is_err());
        assert!(RepoRef::parse("local:~x").is_err());
        assert_eq!(RepoRef::parse("github:a/b").unwrap().local_path(), None);
    }

    #[test]
    fn built_links_stay_readable() {
        let link = ChorongLink::Open {
            repo: RepoRef::GitHub {
                owner: "acme".into(),
                name: "api".into(),
            },
            pr: None,
            base: None,
            side: None,
            rev: None,
            path: Some("src/a b&c.rs".into()),
            symbol: Some("Queue::push".into()),
            line: None,
            branch: None,
            dirty: false,
        };
        assert_eq!(
            link.to_url(),
            "chorong://open?repo=github:acme/api&path=src/a%20b%26c.rs&symbol=Queue::push"
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert!(matches!(
            ChorongLink::parse("https://x"),
            Err(LinkError::Scheme(_))
        ));
        assert!(matches!(
            ChorongLink::parse("chorong://edit?repo=github:a/b"),
            Err(LinkError::Action(_))
        ));
        assert_eq!(
            ChorongLink::parse("chorong://open"),
            Err(LinkError::Missing("repo"))
        );
        for bad in [
            "chorong://open?repo=github:a",
            "chorong://open?repo=gitlab:a/b",
            "chorong://open?repo=local:relative/path",
            "chorong://open?repo=local:/a/../b",
            "chorong://open?repo=github:a/b&path=../etc/passwd",
            "chorong://open?repo=github:a/b&path=/abs",
            "chorong://open?repo=github:a/b&line=0",
            "chorong://open?repo=github:a/b&dirty=yes",
            "chorong://open?repo=github:a/b&path=a//b",
            "chorong://open?repo=github:a/b&path=a/",
            // `rev` and `branch` reach git and a copyable shell command.
            "chorong://open?repo=github:a/b&rev=--upload-pack=touch%20x",
            "chorong://open?repo=github:a/b&rev=-x",
            "chorong://open?repo=github:a/b&rev=",
            "chorong://open?repo=github:a/b&rev=HEAD~1",
            "chorong://open?repo=github:a/b&branch=a%20b",
            "chorong://open?repo=github:a/b&branch=x%0Aecho%20y",
            "chorong://open?repo=github:a/b&branch=x..y",
            "chorong://open?repo=github:a/b&branch=x/.hidden",
            "chorong://open?repo=github:a/b&branch=x.lock",
            "chorong://open?repo=github:a/b&branch=x@%7B1%7D",
        ] {
            assert!(
                matches!(ChorongLink::parse(bad), Err(LinkError::Invalid { .. })),
                "{bad}"
            );
        }
    }

    /// Names git allows keep working, shell metacharacters included: quoting is the job of
    /// whoever builds a command from them (`cho_git::switch_command`).
    #[test]
    fn accepts_commit_ids_and_legal_branch_names() {
        for (rev, branch) in [
            ("3f9c1e2", "main"),
            ("0123456789abcdef0123456789abcdef01234567", "feature/x-1_2"),
            ("abc1234", "x;touch$(id)"),
            ("abc1234", "it's"),
        ] {
            let url = ChorongLink::Open {
                repo: RepoRef::parse("github:a/b").unwrap(),
                pr: None,
                rev: Some(rev.into()),
                base: None,
                side: None,
                path: None,
                symbol: None,
                line: None,
                branch: Some(branch.into()),
                dirty: false,
            }
            .to_url();
            let ChorongLink::Open {
                rev: r, branch: b, ..
            } = ChorongLink::parse(&url).unwrap()
            else {
                panic!("{url}");
            };
            assert_eq!(
                (r.as_deref(), b.as_deref()),
                (Some(rev), Some(branch)),
                "{url}"
            );
        }
    }

    const BASE: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn pull_request_parameters_parse_and_round_trip() {
        let url = format!(
            "chorong://open?repo=github:acme/api&pr=123&rev=aaaaaaa&base={BASE}&side=before&path=src/lib.rs&symbol=Queue::push"
        );
        let link = ChorongLink::parse(&url).unwrap();
        let ChorongLink::Open {
            pr,
            rev,
            base,
            side,
            ..
        } = &link
        else {
            panic!()
        };
        assert_eq!(*pr, Some(123));
        assert_eq!(rev.as_deref(), Some("aaaaaaa"));
        assert_eq!(base.as_deref(), Some(BASE));
        assert_eq!(*side, Some(LinkSide::Before));
        assert_eq!(link.to_url(), url);
        assert_eq!(ChorongLink::parse(&link.to_url()).unwrap(), link);
        // Upper-case hex is accepted and normalised; `side=after` is explicit, not implied.
        let up = ChorongLink::parse(&format!(
            "chorong://open?repo=github:a/b&base={}&side=after",
            BASE.to_ascii_uppercase()
        ))
        .unwrap();
        let ChorongLink::Open { base, side, .. } = up else {
            panic!()
        };
        assert_eq!(base.as_deref(), Some(BASE));
        assert_eq!(side, Some(LinkSide::After));
    }

    #[test]
    fn old_links_have_no_pull_request_parameters() {
        let old = "chorong://open?repo=github:a/b&rev=abc1234&path=src/a.rs&line=3";
        let ChorongLink::Open { pr, base, side, .. } = ChorongLink::parse(old).unwrap() else {
            panic!()
        };
        assert_eq!((pr, base, side), (None, None, None));
        assert_eq!(ChorongLink::parse(old).unwrap().to_url(), old);
    }

    #[test]
    fn malformed_pull_request_parameters_are_rejected() {
        for bad in [
            "chorong://open?repo=github:a/b&pr=abc",
            "chorong://open?repo=github:a/b&pr=0",
            "chorong://open?repo=github:a/b&pr=-3",
            "chorong://open?repo=github:a/b&pr=%2B3",
            "chorong://open?repo=github:a/b&pr=",
            "chorong://open?repo=github:a/b&base=abc1234",
            "chorong://open?repo=github:a/b&base=zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
            "chorong://open?repo=github:a/b&base=0123456789abcdef0123456789abcdef0123456",
            "chorong://open?repo=github:a/b&base=",
            "chorong://open?repo=github:a/b&side=middle",
            "chorong://open?repo=github:a/b&side=",
            "chorong://open?repo=github:a/b&side=Before",
        ] {
            assert!(
                matches!(ChorongLink::parse(bad), Err(LinkError::Invalid { .. })),
                "{bad}"
            );
        }
    }

    #[test]
    fn branch_and_dirty_parameters() {
        let link = ChorongLink::parse(
            "chorong://open?repo=github:a/b&rev=abc1234&branch=feature/x&dirty=1",
        )
        .unwrap();
        let ChorongLink::Open { branch, dirty, .. } = &link else {
            panic!()
        };
        assert_eq!(branch.as_deref(), Some("feature/x"));
        assert!(*dirty);
        assert_eq!(
            link.to_url(),
            "chorong://open?repo=github:a/b&rev=abc1234&branch=feature/x&dirty=1"
        );
        let ChorongLink::Open { dirty, .. } =
            ChorongLink::parse("chorong://open?repo=github:a/b").unwrap()
        else {
            panic!()
        };
        assert!(!dirty);
        let clean = ChorongLink::parse("chorong://open?repo=github:a/b&dirty=0").unwrap();
        assert!(!clean.to_url().contains("dirty"));
    }
}
