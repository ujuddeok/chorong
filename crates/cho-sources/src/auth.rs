//! Host tokens: the OS keychain first (`keyring`), then environment variables. There is no
//! plain-text store. A token is never printed, logged, written to a file or put on a command
//! line; [`Credential`]'s `Debug` hides it.

use crate::{Error, Result};
use cho_core::RepoRef;
use std::fmt;

/// The keychain service name Chorong's entries are stored under.
pub const KEYRING_SERVICE: &str = "chorong";

/// A host Chorong talks to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Host {
    GitHub,
    BitbucketCloud,
    /// Bitbucket Data Center, by host name.
    BitbucketDc(String),
}

impl Host {
    pub fn of(repo: &RepoRef) -> Option<Self> {
        match repo {
            RepoRef::GitHub { .. } => Some(Self::GitHub),
            RepoRef::Bitbucket { .. } => Some(Self::BitbucketCloud),
            RepoRef::BitbucketDc { host, .. } => Some(Self::BitbucketDc(host.clone())),
            RepoRef::Local { .. } => None,
        }
    }

    /// `github.com`, `bitbucket.org`, or any other host name (Data Center).
    pub fn parse(name: &str) -> Result<Self> {
        let n = name.trim().trim_end_matches('/').to_ascii_lowercase();
        let n = n
            .strip_prefix("https://")
            .or_else(|| n.strip_prefix("http://"))
            .unwrap_or(&n)
            .to_string();
        match n.as_str() {
            "github.com" | "github" | "www.github.com" | "api.github.com" => Ok(Self::GitHub),
            "bitbucket.org" | "bitbucket" | "www.bitbucket.org" | "api.bitbucket.org" => {
                Ok(Self::BitbucketCloud)
            }
            "" => Err(Error::msg("expected a host name such as github.com")),
            h if h.contains('/') || h.contains(char::is_whitespace) => Err(Error::msg(format!(
                "`{name}` is not a host name (expected e.g. github.com, bitbucket.org or git.example.com)"
            ))),
            h => Ok(Self::BitbucketDc(h.to_string())),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::GitHub => "github.com",
            Self::BitbucketCloud => "bitbucket.org",
            Self::BitbucketDc(h) => h,
        }
    }

    /// The ref the host keeps pull request `n`'s head at. `None` for Bitbucket Cloud, which
    /// has no pull-request refs (its pull requests are opened from their source branch).
    pub fn pr_ref(&self, n: u64) -> Option<String> {
        match self {
            Self::GitHub => Some(format!("refs/pull/{n}/head")),
            Self::BitbucketDc(_) => Some(format!("refs/pull-requests/{n}/from")),
            Self::BitbucketCloud => None,
        }
    }

    /// The host REST calls go to when no override is set.
    pub fn api_host(&self) -> &str {
        match self {
            Self::GitHub => "api.github.com",
            Self::BitbucketCloud => "api.bitbucket.org",
            Self::BitbucketDc(h) => h,
        }
    }

    /// The variable holding a token when there is no keychain.
    pub fn token_env(&self) -> &'static str {
        match self {
            Self::GitHub => "CHORONG_GITHUB_TOKEN",
            Self::BitbucketCloud => "CHORONG_BITBUCKET_TOKEN",
            Self::BitbucketDc(_) => "CHORONG_BITBUCKET_DC_TOKEN",
        }
    }

    /// Bitbucket Cloud's REST calls need the account email with the token.
    pub fn email_env(&self) -> Option<&'static str> {
        matches!(self, Self::BitbucketCloud).then_some("CHORONG_BITBUCKET_EMAIL")
    }

    /// The user name git sends with the token over HTTPS.
    pub fn git_username(&self) -> &'static str {
        match self {
            Self::GitHub => "x-access-token",
            Self::BitbucketCloud => "x-bitbucket-api-token-auth",
            Self::BitbucketDc(_) => "x-token-auth",
        }
    }
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Where a token came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    Keychain,
    Env(&'static str),
}

impl fmt::Display for TokenSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keychain => f.write_str("keychain"),
            Self::Env(v) => write!(f, "environment ({v})"),
        }
    }
}

/// A token and, for Bitbucket Cloud, the account email it belongs to.
#[derive(Clone)]
pub struct Credential {
    pub token: String,
    pub email: Option<String>,
    pub source: TokenSource,
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("token", &"<hidden>")
            .field("email", &self.email.as_ref().map(|_| "<set>"))
            .field("source", &self.source)
            .finish()
    }
}

/// A place secrets are kept. The real one is the OS keychain; tests use a map.
pub trait SecretStore {
    /// `Err` with a sentence when the store itself is unavailable.
    fn available(&self) -> std::result::Result<(), String>;
    fn get(&self, account: &str) -> std::result::Result<Option<String>, String>;
    fn set(&self, account: &str, secret: &str) -> std::result::Result<(), String>;
    /// `Ok(false)` when there was nothing to delete.
    fn delete(&self, account: &str) -> std::result::Result<bool, String>;
}

/// The OS keychain (macOS Keychain, Windows Credential Manager, Secret Service on Linux).
pub struct Keychain;

impl SecretStore for Keychain {
    fn available(&self) -> std::result::Result<(), String> {
        keyring::Entry::store_status()
            .as_ref()
            .map(|_| ())
            .map_err(|e| {
                format!(
                    "no keychain is available ({e}); on Linux this needs a Secret Service such as \
                 GNOME Keyring or KWallet. Use the environment variable instead"
                )
            })
    }

    fn get(&self, account: &str) -> std::result::Result<Option<String>, String> {
        self.available()?;
        let e = keyring::Entry::new(KEYRING_SERVICE, account).map_err(|e| e.to_string())?;
        match e.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, account: &str, secret: &str) -> std::result::Result<(), String> {
        self.available()?;
        keyring::Entry::new(KEYRING_SERVICE, account)
            .and_then(|e| e.set_password(secret))
            .map_err(|e| e.to_string())
    }

    fn delete(&self, account: &str) -> std::result::Result<bool, String> {
        self.available()?;
        let e = keyring::Entry::new(KEYRING_SERVICE, account).map_err(|e| e.to_string())?;
        match e.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(e.to_string()),
        }
    }
}

fn email_account(host: &Host) -> String {
    format!("{} email", host.name())
}

/// Tokens from a secret store, then the environment.
pub struct Tokens<'a> {
    pub store: &'a dyn SecretStore,
    pub env: &'a dyn Fn(&str) -> Option<String>,
}

/// The process environment, ignoring empty values.
pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// What `cho auth status` reports for a host, without the token.
#[derive(Debug, Clone)]
pub struct AuthStatus {
    pub host: Host,
    pub source: Option<TokenSource>,
    pub has_email: bool,
    /// Why the keychain could not be read, when it couldn't.
    pub keychain_error: Option<String>,
}

impl Tokens<'_> {
    /// The token for `host`: keychain first, then the environment. A keychain that can't be
    /// reached is not an error here; the environment is tried.
    pub fn get(&self, host: &Host) -> Option<Credential> {
        self.lookup(host).0
    }

    fn lookup(&self, host: &Host) -> (Option<Credential>, Option<String>) {
        let mut keychain_error = None;
        match self.store.get(host.name()) {
            Ok(Some(token)) => {
                let email = self
                    .store
                    .get(&email_account(host))
                    .ok()
                    .flatten()
                    .or_else(|| host.email_env().and_then(|v| (self.env)(v)));
                return (
                    Some(Credential {
                        token,
                        email,
                        source: TokenSource::Keychain,
                    }),
                    None,
                );
            }
            Ok(None) => {}
            Err(e) => keychain_error = Some(e),
        }
        let cred = (self.env)(host.token_env()).map(|token| Credential {
            token: token.trim().to_string(),
            email: host.email_env().and_then(|v| (self.env)(v)),
            source: TokenSource::Env(host.token_env()),
        });
        (cred, keychain_error)
    }

    pub fn status(&self, host: &Host) -> AuthStatus {
        let (cred, keychain_error) = self.lookup(host);
        AuthStatus {
            host: host.clone(),
            source: cred.as_ref().map(|c| c.source),
            has_email: cred.as_ref().is_some_and(|c| c.email.is_some()),
            keychain_error,
        }
    }

    /// Stores a token (and email) in the keychain. Fails, saying which variable to use
    /// instead, when there is no keychain. Needs no terminal: the caller passes the token in
    /// (the CLI reads it from standard input; the viewer from a password field).
    pub fn add(&self, host: &Host, token: &str, email: Option<&str>) -> Result<()> {
        let token = token.trim();
        if token.is_empty() {
            return Err(Error::msg("the token is empty"));
        }
        if token.contains(char::is_whitespace) {
            return Err(Error::msg("a token has no spaces or line breaks"));
        }
        if matches!(host, Host::BitbucketCloud) && email.is_none() {
            return Err(Error::msg(
                "Bitbucket Cloud tokens are used with the account email: pass --email",
            ));
        }
        let no_store = |e: String| {
            Error::msg(format!(
                "{e}: set {}{} in the environment Chorong runs in",
                host.token_env(),
                host.email_env()
                    .map(|v| format!(" and {v}"))
                    .unwrap_or_default()
            ))
        };
        self.store.set(host.name(), token).map_err(no_store)?;
        if let Some(e) = email {
            self.store
                .set(&email_account(host), e.trim())
                .map_err(no_store)?;
        }
        Ok(())
    }

    /// Removes the keychain entries for `host`. `Ok(false)` when there were none. Needs no
    /// terminal.
    pub fn remove(&self, host: &Host) -> Result<bool> {
        let removed = self.store.delete(host.name()).map_err(Error::msg)?;
        let _ = self.store.delete(&email_account(host));
        Ok(removed)
    }
}

/// Answers one `git credential` request (git's stdin protocol: `key=value` lines). Returns
/// the lines to print: `username` and `password` when the request is HTTPS to a host Chorong
/// has a token for, else nothing (git then moves on).
pub fn credential_response(operation: &str, request: &str, tokens: &Tokens) -> String {
    if operation != "get" {
        return String::new();
    }
    let field = |k: &str| {
        request
            .lines()
            .find_map(|l| l.strip_prefix(k)?.strip_prefix('='))
            .map(str::trim)
    };
    if field("protocol") != Some("https") {
        return String::new();
    }
    let Some(host) = field("host") else {
        return String::new();
    };
    // `host` may carry a port.
    let name = host.split(':').next().unwrap_or(host);
    let Ok(host) = Host::parse(name) else {
        return String::new();
    };
    match tokens.get(&host) {
        Some(c) => format!("username={}\npassword={}\n", host.git_username(), c.token),
        None => String::new(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    pub struct MapStore {
        pub map: RefCell<BTreeMap<String, String>>,
        pub broken: bool,
    }

    impl SecretStore for MapStore {
        fn available(&self) -> std::result::Result<(), String> {
            if self.broken {
                Err("no keychain".into())
            } else {
                Ok(())
            }
        }
        fn get(&self, a: &str) -> std::result::Result<Option<String>, String> {
            self.available()?;
            Ok(self.map.borrow().get(a).cloned())
        }
        fn set(&self, a: &str, s: &str) -> std::result::Result<(), String> {
            self.available()?;
            self.map.borrow_mut().insert(a.into(), s.into());
            Ok(())
        }
        fn delete(&self, a: &str) -> std::result::Result<bool, String> {
            self.available()?;
            Ok(self.map.borrow_mut().remove(a).is_some())
        }
    }

    #[test]
    fn hosts() {
        assert_eq!(Host::parse("github.com").unwrap(), Host::GitHub);
        assert_eq!(
            Host::parse("https://bitbucket.org/").unwrap(),
            Host::BitbucketCloud
        );
        assert_eq!(
            Host::parse("git.example.com").unwrap(),
            Host::BitbucketDc("git.example.com".into())
        );
        assert!(Host::parse("a/b").is_err());
    }

    /// Bitbucket Cloud has no pull-request refs: the open falls back to the source branch
    /// rather than asking for GitHub's `refs/pull/N/head`.
    #[test]
    fn pull_request_refs_per_host() {
        assert_eq!(Host::GitHub.pr_ref(3).as_deref(), Some("refs/pull/3/head"));
        assert_eq!(
            Host::BitbucketDc("h".into()).pr_ref(3).as_deref(),
            Some("refs/pull-requests/3/from")
        );
        assert_eq!(Host::BitbucketCloud.pr_ref(3), None);
    }

    #[test]
    fn keychain_then_env() {
        let store = MapStore::default();
        let env = |k: &str| (k == "CHORONG_GITHUB_TOKEN").then(|| "envtok".to_string());
        let t = Tokens {
            store: &store,
            env: &env,
        };
        let c = t.get(&Host::GitHub).unwrap();
        assert_eq!(c.source, TokenSource::Env("CHORONG_GITHUB_TOKEN"));
        t.add(&Host::GitHub, "kctok\n", None).unwrap();
        let c = t.get(&Host::GitHub).unwrap();
        assert_eq!(
            (c.token.as_str(), c.source),
            ("kctok", TokenSource::Keychain)
        );
        assert!(!format!("{c:?}").contains("kctok"));
        assert!(t.remove(&Host::GitHub).unwrap());
        assert!(!t.remove(&Host::GitHub).unwrap());
        assert!(t.add(&Host::BitbucketCloud, "x", None).is_err());
    }

    #[test]
    fn broken_keychain_falls_back() {
        let store = MapStore {
            broken: true,
            ..Default::default()
        };
        let env = |k: &str| match k {
            "CHORONG_BITBUCKET_TOKEN" => Some("t".to_string()),
            "CHORONG_BITBUCKET_EMAIL" => Some("e".to_string()),
            _ => None,
        };
        let t = Tokens {
            store: &store,
            env: &env,
        };
        let s = t.status(&Host::BitbucketCloud);
        assert_eq!(s.source, Some(TokenSource::Env("CHORONG_BITBUCKET_TOKEN")));
        assert!(s.has_email && s.keychain_error.is_some());
        let err = t.add(&Host::GitHub, "x", None).unwrap_err().to_string();
        assert!(err.contains("CHORONG_GITHUB_TOKEN"), "{err}");
    }

    #[test]
    fn credential_protocol() {
        let store = MapStore::default();
        let env = |k: &str| (k == "CHORONG_GITHUB_TOKEN").then(|| "tok".to_string());
        let t = Tokens {
            store: &store,
            env: &env,
        };
        let r = credential_response("get", "protocol=https\nhost=github.com\n\n", &t);
        assert_eq!(r, "username=x-access-token\npassword=tok\n");
        assert_eq!(
            credential_response("get", "protocol=ssh\nhost=github.com\n", &t),
            ""
        );
        assert_eq!(
            credential_response("store", "protocol=https\nhost=github.com\n", &t),
            ""
        );
        assert_eq!(
            credential_response("get", "protocol=https\nhost=bitbucket.org\n", &t),
            ""
        );
    }
}
