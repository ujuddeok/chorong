//! The environment Chorong gives the project's own code (indexers, prepare steps): what a build
//! needs, and none of the tokens and keys exported in the reader's shell.

/// Environment variables a build may see. Indexers and prepare steps run the project's own code (build
/// scripts, procedural macros, npm packages), so they get what a build needs (paths, toolchain,
/// compiler and proxy settings, locale) and nothing else: no tokens or keys exported in the
/// reader's shell (`GITHUB_TOKEN`, `AWS_*`, `NPM_TOKEN`, `npm_config__authToken`,
/// `CARGO_REGISTRIES_*_TOKEN`, `SSH_AUTH_SOCK`, ...). This does not stop a build script from
/// reading files the reader can read; it only keeps secrets out of easy reach.
const ENV_ALLOWED: &[&str] = &[
    // System and locale
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TMPDIR",
    "TMP",
    "TEMP",
    "LANG",
    "LANGUAGE",
    "TZ",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    // Proxies and certificates (needed to download crates and npm packages)
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "CARGO_HTTP_CAINFO",
    // Rust
    "CARGO_HOME",
    "CARGO_TARGET_DIR",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTFLAGS",
    "RUSTDOCFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    // C toolchains used by build scripts
    "CC",
    "CXX",
    "AR",
    "CFLAGS",
    "CXXFLAGS",
    "LDFLAGS",
    "PKG_CONFIG",
    "PKG_CONFIG_PATH",
    "LIBCLANG_PATH",
    "CLANG_PATH",
    "BINDGEN_EXTRA_CLANG_ARGS",
    "LD_LIBRARY_PATH",
    "SDKROOT",
    "DEVELOPER_DIR",
    "MACOSX_DEPLOYMENT_TARGET",
    // Node and Python
    "NODE_PATH",
    // The reader's own heap limit and flags; Chorong adds a heap limit only when none is set.
    "NODE_OPTIONS",
    "NVM_DIR",
    "NPM_CONFIG_REGISTRY",
    "npm_config_registry",
    "NPM_CONFIG_CACHE",
    "npm_config_cache",
    "VIRTUAL_ENV",
    "PYTHONPATH",
    "PYENV_ROOT",
    "CONDA_PREFIX",
    // uv's caches (prepare's `uv sync`); no credentials
    "UV_CACHE_DIR",
    "UV_PYTHON_INSTALL_DIR",
];

/// Prefixes of allowed families that carry no secrets.
const ENV_ALLOWED_PREFIXES: &[&str] = &["LC_", "RUSTUP_", "CARGO_BUILD_", "CARGO_NET_"];

/// True for a variable a build may see.
pub fn allowed(name: &str) -> bool {
    ENV_ALLOWED.contains(&name) || ENV_ALLOWED_PREFIXES.iter().any(|p| name.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::allowed;

    /// A literal table, not the lists above: a name added to or dropped from the lists, or a
    /// prefix match that loosens, shows up here.
    #[test]
    fn builds_see_toolchain_settings_and_no_secrets() {
        for name in [
            "PATH",
            "HOME",
            "HTTPS_PROXY",
            "SSL_CERT_FILE",
            "CARGO_HOME",
            "RUSTFLAGS",
            "CC",
            "PKG_CONFIG_PATH",
            "NODE_OPTIONS",
            "VIRTUAL_ENV",
            "LC_ALL",
            "RUSTUP_DIST_SERVER",
            "CARGO_BUILD_JOBS",
            "CARGO_NET_OFFLINE",
        ] {
            assert!(allowed(name), "{name} should be allowed");
        }
        for name in [
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "NPM_TOKEN",
            "npm_config__authToken",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "CARGO_REGISTRIES_MY_TOKEN",
            "CARGO_REGISTRY_TOKEN",
            "SSH_AUTH_SOCK",
            "ANTHROPIC_API_KEY",
            "GIT_ASKPASS",
            // Prefixes match at the start only, and case matters.
            "MY_CARGO_NET_TOKEN",
            "XLC_SECRET",
            "path",
            "lc_all",
            "",
        ] {
            assert!(!allowed(name), "{name:?} should be refused");
        }
    }
}
