//! The viewer's small JSON files in Chorong's config directory (`cho_settings::config_dir`):
//! `trusted.json`, `graph.json` and `remote.json`. Never in a repository.
//!
//! A missing or unreadable file reads as the default. Saving writes a temporary file beside the
//! target and renames it over, so a crash mid-write never leaves a torn file, and returns the
//! error for the caller to show.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};

/// `name` in Chorong's config directory; `None` when there is no home to put it in.
pub fn file(name: &str) -> Option<PathBuf> {
    cho_settings::config_dir().map(|d| d.join(name))
}

/// The value in `path`, or the default when the file is missing or not valid JSON for `T`.
#[allow(clippy::disallowed_methods)] // Chorong's own settings file
pub fn load<T: DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Writes `value` to `path` as pretty JSON, atomically (temporary file, then rename), creating
/// the directory when needed.
pub fn save<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    let tmp = temp_name(path);
    let written = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    written
}

/// A temporary name beside `path` that no other save uses: two saves at once (two threads,
/// or two windows' processes) never write or rename each other's file.
fn temp_name(path: &Path) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.{n}.tmp", std::process::id()));
    PathBuf::from(tmp)
}

/// [`save`] to `name` in the config directory, with the error as a sentence for the reader.
pub fn save_named<T: Serialize>(name: &str, value: &T) -> Result<(), String> {
    let path = file(name).ok_or_else(|| format!("cannot save {name}: no config directory"))?;
    save(&path, value).map_err(|e| format!("cannot save {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Files left beside the target other than the target itself.
    fn leftovers(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name() != "p.json")
            .count()
    }

    #[test]
    fn saves_at_once_from_several_threads_never_collide() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/p.json");
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for j in 0..25 {
                        let value = BTreeMap::from([(format!("{i}-{j}"), true)]);
                        save(&path, &value).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(
            load::<BTreeMap<String, bool>>(&path).len(),
            1,
            "one whole file"
        );
        assert_eq!(
            leftovers(&dir.path().join("nested")),
            0,
            "no temporary files"
        );
    }

    #[test]
    fn saves_atomically_and_reads_back_or_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/p.json");
        let empty: BTreeMap<String, bool> = load(&path);
        assert!(empty.is_empty(), "missing file is the default");
        let value = BTreeMap::from([("a".to_string(), true)]);
        save(&path, &value).unwrap();
        assert_eq!(load::<BTreeMap<String, bool>>(&path), value);
        assert_eq!(leftovers(&dir.path().join("nested")), 0);
        std::fs::write(&path, "not json").unwrap();
        assert!(load::<BTreeMap<String, bool>>(&path).is_empty());
    }
}
