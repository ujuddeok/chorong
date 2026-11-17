//! The viewer's whole use of `cho-sources`, behind a few plain functions and types.
//!
//! Everything here blocks (git, REST, the keychain, the repository lock): call it from the
//! background executor, never the UI thread. `cho_sources::Sources` is not `Send`, so it is
//! built inside each call, on whichever thread calls it. Errors are the backend's sentences as
//! plain strings.
//!
//! Only this module touches `cho-sources`. [`open`] opens a project (and reads the header
//! band's facts), [`prepare`] assesses and runs "Prepare and index", and [`settings`] is the
//! settings window's Projects, Cache and Accounts backend.

mod open;
mod prepare;
mod settings;

pub use cho_sources::Cancel;
pub use open::*;
pub use prepare::*;
pub use settings::*;

use cho_core::RepoRef;
use cho_sources::auth::{Keychain, SecretStore, process_env};
use cho_sources::{Api, Cache, Git, Projects, Sources, Tokens};
use std::path::{Path, PathBuf};

/// The remembered projects.
pub fn projects() -> Vec<RepoRef> {
    Projects::open_default()
        .map(|p| p.list().into_iter().map(|(r, _)| r).collect())
        .unwrap_or_default()
}

fn build_sources<'a>(cache: Cache, backend: Backend<'a>) -> Sources<'a> {
    Sources {
        cache,
        git: backend.git,
        tokens: Tokens {
            store: backend.store,
            env: backend.env,
        },
        api: backend.api,
        lock_timeout: cho_sources::LOCK_TIMEOUT,
        index_cache: cho_semantic::IndexCache::default_location(),
    }
}

/// The project a folder is a cache worktree of, if any.
pub fn worktree_project(folder: &Path) -> Option<RepoRef> {
    cache()?.worktree_owner(folder).map(|o| o.0)
}

/// Chorong's repository cache, for naming recent folders.
pub fn cache() -> Option<Cache> {
    Cache::default_location()
}

static KEYCHAIN: Keychain = Keychain;

/// Chorong's repository cache, or the sentence that says why there is none.
fn require_cache() -> Result<Cache, String> {
    cache().ok_or_else(|| "cannot find a cache directory (HOME is not set)".into())
}

/// The tokens in the keychain and the environment.
fn keychain_tokens() -> Tokens<'static> {
    Tokens {
        store: &KEYCHAIN,
        env: &process_env,
    }
}

fn real_backend<'a>(helper: Option<PathBuf>) -> Backend<'a> {
    Backend {
        store: &KEYCHAIN,
        env: &process_env,
        git: Git {
            credential_helper: helper,
        },
        api: Api::from_env(),
    }
}

/// [`build_sources`] with the real backend. `helper` is the git credential helper, for calls
/// that fetch.
fn real_sources(cache: Cache, helper: Option<PathBuf>) -> Sources<'static> {
    build_sources(cache, real_backend(helper))
}

/// What `open_in` talks to; the real ones in [`open`], local ones in tests.
struct Backend<'a> {
    store: &'a dyn SecretStore,
    env: &'a dyn Fn(&str) -> Option<String>,
    git: Git,
    api: Api,
}

#[cfg(test)]
mod test_support {
    use super::*;
    use cho_sources::TargetKind;

    struct NoKeychain;

    impl SecretStore for NoKeychain {
        fn available(&self) -> Result<(), String> {
            Err("no keychain in tests".into())
        }
        fn get(&self, _: &str) -> Result<Option<String>, String> {
            Ok(None)
        }
        fn set(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<bool, String> {
            Ok(false)
        }
    }

    pub(super) fn backend<'a>() -> Backend<'a> {
        static STORE: NoKeychain = NoKeychain;
        Backend {
            store: &STORE,
            env: &|_| None,
            git: Git::default(),
            // Nothing listens here: a branch open must not need the host's REST API.
            api: Api {
                github: "http://127.0.0.1:9".into(),
                bitbucket: "http://127.0.0.1:9".into(),
            },
        }
    }

    pub(super) fn request(url: &str, kind: TargetKind) -> Request {
        Request {
            repo: RepoRef::parse("github:acme/api").unwrap(),
            clone_url: Some(url.into()),
            kind,
            path: Some("src/a.rs".into()),
            line: Some(3),
            base: None,
        }
    }

    pub(super) fn sources_in(root: &Path) -> Sources<'static> {
        static STORE: NoKeychain = NoKeychain;
        Sources {
            cache: Cache::at(root),
            git: Git::default(),
            tokens: Tokens {
                store: &STORE,
                env: &|_| None,
            },
            api: Api::default(),
            lock_timeout: std::time::Duration::from_secs(5),
            index_cache: None,
        }
    }
}
