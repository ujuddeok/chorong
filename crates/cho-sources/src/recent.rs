//! `recent.json` in Chorong's config directory: folders opened most recently first, so `chorong://`
//! links that name a repository by its remote resolve to a worktree on this machine. The
//! viewer reads and writes the same file through this module; `cho open` adds the cache worktrees it places, so remote and
//! pull-request links find them.

use std::path::{Path, PathBuf};

/// Most folders remembered (the viewer's limit).
const KEEP: usize = 40;

/// The folders in a file's text, most recent first, each once (a hand-edited file may repeat
/// one). Entries that aren't strings are skipped, so one bad entry doesn't cost the rest (the
/// next `remember` would otherwise rewrite the file without them). `None` when it isn't a
/// readable `recent.json`.
fn parse(text: &str) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let mut seen = std::collections::HashSet::new();
    let recent = match value.as_object()?.get("recent") {
        None => vec![],
        Some(r) => r
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::to_string)
            .collect(),
    };
    Some(
        recent
            .into_iter()
            .filter(|r| seen.insert(r.clone()))
            .collect(),
    )
}

/// Folders opened before, most recent first. Empty when there is no file.
pub fn list() -> Vec<PathBuf> {
    cho_settings::config_dir()
        .map(|d| list_in(&d.join("recent.json")))
        .unwrap_or_default()
}

#[allow(clippy::disallowed_methods)] // Chorong's own recent.json
pub(crate) fn list_in(path: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| parse(&t))
        .map(|recent| recent.into_iter().map(PathBuf::from).collect())
        .unwrap_or_default()
}

/// Records `folder` (canonicalised) as the most recently opened. Best effort.
pub fn remember(folder: &Path) -> std::io::Result<()> {
    let Some(dir) = cho_settings::config_dir() else {
        return Err(std::io::Error::other("no config directory"));
    };
    remember_in(&dir.join("recent.json"), folder)
}

#[allow(clippy::disallowed_methods)] // Chorong's own recent.json
pub(crate) fn remember_in(path: &Path, folder: &Path) -> std::io::Result<()> {
    let key = folder
        .canonicalize()
        .unwrap_or_else(|_| folder.to_path_buf())
        .to_string_lossy()
        .into_owned();
    let mut recent = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| parse(&t))
        .unwrap_or_default();
    recent.retain(|r| *r != key);
    recent.insert(0, key);
    recent.truncate(KEEP);
    crate::write_atomic(
        path,
        serde_json::to_string_pretty(&serde_json::json!({ "recent": recent }))
            .expect("json")
            .as_bytes(),
    )
    .map_err(|e| std::io::Error::other(e.to_string()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)] // reads the file it wrote
    use super::*;

    #[test]
    fn most_recent_first_in_the_viewers_format() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("recent.json");
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        remember_in(&f, &a).unwrap();
        remember_in(&f, &b).unwrap();
        remember_in(&f, &a).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        let list: Vec<&str> = v["recent"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert_eq!(list.len(), 2);
        assert_eq!(list_in(&f).len(), 2);
        assert!(list_in(&d.path().join("none.json")).is_empty());
        assert!(list[0].ends_with("/a") && list[1].ends_with("/b"));
    }

    /// Found by the `persist_state` fuzz target: a folder repeated in a hand-edited file was
    /// listed (and kept) twice.
    /// Found by reviewing the `persist_state` fuzz target: one entry that wasn't a string made
    /// the whole file unreadable, and the next `remember` rewrote it with only the new folder.
    #[test]
    fn a_bad_entry_does_not_cost_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("recent.json");
        std::fs::write(&f, r#"{"recent": ["/a", 7, null, "/b"]}"#).unwrap();
        assert_eq!(
            super::list_in(&f),
            [PathBuf::from("/a"), PathBuf::from("/b")]
        );
        super::remember_in(&f, Path::new("/c")).unwrap();
        let names: Vec<PathBuf> = super::list_in(&f);
        assert_eq!(
            names,
            [
                PathBuf::from("/c"),
                PathBuf::from("/a"),
                PathBuf::from("/b")
            ]
        );
    }

    #[test]
    fn repeated_folders_are_listed_once() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("recent.json");
        std::fs::write(&f, r#"{"recent":["/x","/y","/x"]}"#).unwrap();
        assert_eq!(list_in(&f), [PathBuf::from("/x"), PathBuf::from("/y")]);
        let z = d.path().join("z");
        std::fs::create_dir_all(&z).unwrap();
        remember_in(&f, &z).unwrap();
        assert_eq!(list_in(&f).len(), 3);
    }
}
