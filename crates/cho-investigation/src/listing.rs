//! Listing a repository's `.chorong/investigations/*.json`.

use crate::Kind;
use crate::source::{self, Source};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Where agents write investigations, relative to the worktree root.
pub const INVESTIGATIONS_DIR: &str = ".chorong/investigations";
/// At most this many files are listed (the rest are reported as `truncated`).
pub const MAX_LISTED: usize = 500;

/// One `.json` file in the folder.
#[derive(Debug, Clone, Serialize)]
pub struct ListEntry {
    /// Repository-relative path (`.chorong/investigations/NAME.json`).
    pub path: String,
    /// The file to open.
    #[serde(skip)]
    pub source: Source,
    #[serde(flatten)]
    pub info: EntryInfo,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum EntryInfo {
    Valid {
        title: String,
        kind: Kind,
        rev: String,
        nodes: usize,
        steps: usize,
    },
    /// The file can't be read or doesn't parse; `reason` says why.
    Invalid { reason: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct Listing {
    pub entries: Vec<ListEntry>,
    /// More than [`MAX_LISTED`] files: only the first are listed.
    pub truncated: bool,
}

/// Lists `root/.chorong/investigations/*.json`, sorted by file name (byte order). Regular
/// files only: directories are skipped, and a symbolic link that leads out of the
/// worktree is listed with a reason. A missing folder is an empty listing.
pub fn list(root: &Path) -> std::io::Result<Listing> {
    let mut listing = Listing {
        entries: vec![],
        truncated: false,
    };
    let dir = root.join(INVESTIGATIONS_DIR);
    // The folder itself must not lead out of the worktree.
    let (Ok(real_root), Ok(real_dir)) = (root.canonicalize(), dir.canonicalize()) else {
        return Ok(listing);
    };
    if !real_dir.starts_with(&real_root) || !real_dir.is_dir() {
        return Ok(listing);
    }
    let mut names: Vec<(String, PathBuf)> = vec![];
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !name.ends_with(".json") || name.starts_with('.') {
            continue;
        }
        let ty = entry.file_type()?;
        if ty.is_dir() {
            continue;
        }
        names.push((name, entry.path()));
    }
    names.sort();
    listing.truncated = names.len() > MAX_LISTED;
    names.truncate(MAX_LISTED);
    for (name, path) in names {
        let rel = format!("{INVESTIGATIONS_DIR}/{name}");
        let info = match source::read(&Source::Path(path.clone()), Some(root))
            .and_then(|f| source::parse_bytes(&f.bytes))
        {
            Ok(inv) => EntryInfo::Valid {
                title: inv.title,
                kind: inv.kind,
                rev: inv.rev,
                nodes: inv.nodes.len(),
                steps: inv.steps.len(),
            },
            Err(e) => EntryInfo::Invalid {
                reason: e.to_string(),
            },
        };
        // A directory reached through a symbolic link is still not a file.
        if matches!(&info, EntryInfo::Invalid { reason } if reason.contains("not a regular file"))
            && path.is_dir()
        {
            continue;
        }
        listing.entries.push(ListEntry {
            path: rel,
            source: Source::Path(path),
            info,
        });
    }
    Ok(listing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_sorted_with_reasons() {
        let root = std::env::temp_dir().join(format!("cho-list-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join(INVESTIGATIONS_DIR);
        fs::create_dir_all(dir.join("sub.json")).unwrap();
        let good = serde_json::json!({
            "chorong": 1, "title": "Tour", "kind": "tour", "repo": "local:/x", "rev": "abcdef1",
            "nodes": [{ "id": "a", "anchor": { "path": "a.rs" } }], "steps": ["a"]
        });
        fs::write(dir.join("b.json"), good.to_string()).unwrap();
        fs::write(dir.join("a.json"), "{ not json").unwrap();
        fs::write(dir.join("notes.txt"), "x").unwrap();
        let outside =
            std::env::temp_dir().join(format!("cho-list-out-{}.json", std::process::id()));
        fs::write(&outside, good.to_string()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, dir.join("c.json")).unwrap();

        let listing = list(&root).unwrap();
        let names: Vec<&str> = listing.entries.iter().map(|e| e.path.as_str()).collect();
        let mut want = vec![
            ".chorong/investigations/a.json",
            ".chorong/investigations/b.json",
        ];
        if cfg!(unix) {
            want.push(".chorong/investigations/c.json");
        }
        assert_eq!(names, want);
        assert!(matches!(listing.entries[0].info, EntryInfo::Invalid { .. }));
        match &listing.entries[1].info {
            EntryInfo::Valid {
                title, kind, rev, ..
            } => {
                assert_eq!(
                    (title.as_str(), kind, rev.as_str()),
                    ("Tour", &Kind::Tour, "abcdef1")
                );
            }
            other => panic!("{other:?}"),
        }
        if cfg!(unix) {
            match &listing.entries[2].info {
                EntryInfo::Invalid { reason } => assert!(reason.contains("outside"), "{reason}"),
                other => panic!("{other:?}"),
            }
        }
        let json = serde_json::to_value(&listing).unwrap();
        assert_eq!(json["entries"][1]["kind"], "tour");
        assert!(json["entries"][0]["reason"].is_string());

        assert!(list(&root.join("missing")).unwrap().entries.is_empty());
        fs::remove_dir_all(&root).ok();
        fs::remove_file(outside).ok();
    }
}
