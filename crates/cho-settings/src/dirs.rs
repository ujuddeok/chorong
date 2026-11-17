//! Where Chorong keeps its own files: `settings.json`, `repos.json` and `trusted.json`.

use std::ffi::OsString;
use std::path::PathBuf;

/// Resolve the config directory from an environment lookup, so tests don't touch the real one.
///
/// Order: `CHORONG_CONFIG_DIR` (replaces the whole `chorong` directory; for tests and screenshots),
/// then on macOS `~/Library/Application Support/chorong`, elsewhere `$XDG_CONFIG_HOME/chorong`, else
/// `~/.config/chorong`. `None` when there is no home to put it in.
pub fn resolve_config_dir(env: impl Fn(&str) -> Option<OsString>, macos: bool) -> Option<PathBuf> {
    let get = |k: &str| env(k).filter(|v| !v.is_empty());
    if let Some(dir) = get("CHORONG_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = get("HOME").map(PathBuf::from);
    let base = if macos {
        home?.join("Library/Application Support")
    } else {
        match get("XDG_CONFIG_HOME") {
            Some(x) => PathBuf::from(x),
            None => home?.join(".config"),
        }
    };
    Some(base.join("chorong"))
}

/// Chorong's config directory on this machine. Both `settings.json` and `trusted.json` live here.
pub fn config_dir() -> Option<PathBuf> {
    resolve_config_dir(|k| std::env::var_os(k), cfg!(target_os = "macos"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).map(OsString::from)
    }

    #[test]
    fn xdg_then_home() {
        let both = env(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/h")]);
        assert_eq!(resolve_config_dir(both, false), Some("/xdg/chorong".into()));
        assert_eq!(
            resolve_config_dir(env(&[("HOME", "/h")]), false),
            Some("/h/.config/chorong".into())
        );
        // An empty value counts as unset.
        let empty = env(&[("XDG_CONFIG_HOME", ""), ("HOME", "/h")]);
        assert_eq!(
            resolve_config_dir(empty, false),
            Some("/h/.config/chorong".into())
        );
    }

    #[test]
    fn macos_ignores_xdg() {
        let e = env(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/Users/u")]);
        assert_eq!(
            resolve_config_dir(e, true),
            Some("/Users/u/Library/Application Support/chorong".into())
        );
    }

    #[test]
    fn override_wins_and_no_home_is_none() {
        let e = env(&[("CHORONG_CONFIG_DIR", "/t"), ("HOME", "/h")]);
        assert_eq!(resolve_config_dir(e, false), Some("/t".into()));
        assert_eq!(resolve_config_dir(env(&[]), false), None);
        assert_eq!(resolve_config_dir(env(&[]), true), None);
    }
}
