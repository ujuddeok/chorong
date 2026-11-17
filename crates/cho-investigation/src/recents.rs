//! What Chorong remembers about investigations, in its own settings folder and never in the
//! worktree: recently opened investigations per repository, and the current step ("place")
//! per investigation. Only sources, times and step ids are stored, never the content.

use crate::source::Source;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Recents kept per repository.
pub const RECENTS_LIMIT: usize = 20;
/// Places kept in all; the least recently used go first.
pub const PLACES_LIMIT: usize = 200;
/// The file inside the folder the caller passes.
pub const FILE_NAME: &str = "investigations.json";
/// Larger state files are ignored rather than read.
const MAX_STATE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recent {
    pub source: Source,
    /// When it was last opened, in seconds since the Unix epoch.
    pub opened_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    /// The node id of the current step.
    pub step: String,
    pub at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    version: u32,
    /// Repository key → recents, most recent first.
    #[serde(default)]
    recents: BTreeMap<String, Vec<Recent>>,
    /// Source (as text) → place.
    #[serde(default)]
    places: BTreeMap<String, Place>,
}

/// Recents and places, stored as JSON in `dir/investigations.json`. Every call reads the
/// file, so several windows see each other's changes; writes are atomic (write, rename).
#[derive(Debug, Clone)]
pub struct RecentsStore {
    dir: PathBuf,
}

/// The key for a repository: its root path as given (callers pass a canonical root).
fn repo_key(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

fn source_key(source: &Source) -> String {
    match source {
        Source::Path(p) => format!("path:{}", p.display()),
        Source::Url(u) => format!("url:{u}"),
    }
}

impl RecentsStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(FILE_NAME)
    }

    fn load(&self) -> State {
        let path = self.path();
        #[allow(clippy::disallowed_methods)] // Chorong's own settings file, size checked first
        let text = match std::fs::metadata(&path) {
            Ok(m) if m.is_file() && m.len() <= MAX_STATE_BYTES => {
                std::fs::read_to_string(&path).unwrap_or_default()
            }
            _ => String::new(),
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    fn save(&self, state: &State) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.path();
        let tmp = path.with_extension(format!("json.tmp{}", std::process::id()));
        let text = serde_json::to_string_pretty(state).expect("state serialises");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    /// Recents for the repository at `root`, most recent first.
    pub fn recents(&self, root: &Path) -> Vec<Recent> {
        self.load()
            .recents
            .remove(&repo_key(root))
            .unwrap_or_default()
    }

    /// Records that `source` was opened in the repository at `root` at `now` (seconds since
    /// the epoch): it moves to the front, and the list is cut to [`RECENTS_LIMIT`].
    pub fn record_opened(&self, root: &Path, source: &Source, now: u64) -> std::io::Result<()> {
        let mut state = self.load();
        state.version = 1;
        let list = state.recents.entry(repo_key(root)).or_default();
        list.retain(|r| &r.source != source);
        list.insert(
            0,
            Recent {
                source: source.clone(),
                opened_at: now,
            },
        );
        list.truncate(RECENTS_LIMIT);
        self.save(&state)
    }

    /// Removes `source` from the repository's recents (e.g. the reader dismissed a missing one).
    pub fn forget(&self, root: &Path, source: &Source) -> std::io::Result<()> {
        let mut state = self.load();
        if let Some(list) = state.recents.get_mut(&repo_key(root)) {
            list.retain(|r| &r.source != source);
        }
        state.places.remove(&source_key(source));
        self.save(&state)
    }

    /// The step id last current in `source`, if any.
    pub fn place(&self, source: &Source) -> Option<String> {
        self.load()
            .places
            .remove(&source_key(source))
            .map(|p| p.step)
    }

    /// Remembers `step` as the current step of `source`.
    pub fn set_place(&self, source: &Source, step: &str, now: u64) -> std::io::Result<()> {
        let mut state = self.load();
        state.version = 1;
        state.places.insert(
            source_key(source),
            Place {
                step: step.to_string(),
                at: now,
            },
        );
        if state.places.len() > PLACES_LIMIT {
            let mut by_age: Vec<(u64, String)> = state
                .places
                .iter()
                .map(|(k, p)| (p.at, k.clone()))
                .collect();
            by_age.sort();
            for (_, k) in by_age.iter().take(state.places.len() - PLACES_LIMIT) {
                state.places.remove(k);
            }
        }
        self.save(&state)
    }
}

/// Seconds since the Unix epoch, for `now` arguments.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> (RecentsStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("cho-recents-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (RecentsStore::new(&dir), dir)
    }

    #[test]
    fn recents_are_per_repo_most_recent_first_and_capped() {
        let (s, dir) = store("cap");
        let (a, b) = (Path::new("/repo/a"), Path::new("/repo/b"));
        assert!(s.recents(a).is_empty());
        for i in 0..25u64 {
            s.record_opened(a, &Source::Path(format!("/f/{i}.json").into()), i)
                .unwrap();
        }
        let url = Source::url("https://example.com/t.json").unwrap();
        s.record_opened(b, &url, 7).unwrap();
        // Reopening moves it to the front without duplicating it.
        s.record_opened(a, &Source::Path("/f/10.json".into()), 100)
            .unwrap();
        let ra = s.recents(a);
        assert_eq!(ra.len(), RECENTS_LIMIT);
        assert_eq!(ra[0].source, Source::Path("/f/10.json".into()));
        assert_eq!(ra[0].opened_at, 100);
        assert_eq!(ra[1].source, Source::Path("/f/24.json".into()));
        assert_eq!(ra.iter().filter(|r| r.source == ra[0].source).count(), 1);
        assert_eq!(
            s.recents(b),
            vec![Recent {
                source: url.clone(),
                opened_at: 7
            }]
        );
        s.forget(b, &url).unwrap();
        assert!(s.recents(b).is_empty());

        // The file holds sources and times only.
        let text = cho_core::fs::read_source_text(&s.path()).unwrap();
        assert!(text.contains("\"path\": \"/f/10.json\""), "{text}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn places_round_trip_and_are_capped() {
        let (s, dir) = store("place");
        let src = Source::Path("/f/t.json".into());
        assert_eq!(s.place(&src), None);
        s.set_place(&src, "handle", 1).unwrap();
        assert_eq!(s.place(&src).as_deref(), Some("handle"));
        for i in 0..PLACES_LIMIT as u64 + 5 {
            s.set_place(&Source::Path(format!("/p/{i}").into()), "x", 10 + i)
                .unwrap();
        }
        // The oldest (ours) was dropped.
        assert_eq!(s.place(&src), None);
        assert_eq!(
            s.place(&Source::Path(format!("/p/{}", PLACES_LIMIT + 4).into()))
                .as_deref(),
            Some("x")
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_corrupt_file_is_an_empty_store() {
        let (s, dir) = store("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(s.path(), "{ nope").unwrap();
        assert!(s.recents(Path::new("/r")).is_empty());
        s.record_opened(Path::new("/r"), &Source::Path("/x.json".into()), 1)
            .unwrap();
        assert_eq!(s.recents(Path::new("/r")).len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }
}
