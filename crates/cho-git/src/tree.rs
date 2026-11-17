//! Comparing two commits' trees from the object store: changed files (with exact and
//! similarity renames), blob contents, and merge bases. Nothing reads or moves a checkout.

use crate::linediff::similarity;
use crate::short;
use gix::ObjectId;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How a file changed between two trees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

impl FileStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
        }
    }
}

/// What a tree entry holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    /// A regular or executable file.
    File,
    Symlink,
    /// A gitlink (submodule commit); there are no contents to read.
    Submodule,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Symlink => "symlink",
            Self::Submodule => "submodule",
        }
    }
}

/// One side of a changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSide {
    /// Repository-relative, `/`-separated.
    pub path: String,
    pub id: ObjectId,
    pub kind: EntryKind,
    pub executable: bool,
}

/// One changed file. `base` is absent for an added file, `head` for a deleted one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub status: FileStatus,
    pub base: Option<FileSide>,
    pub head: Option<FileSide>,
    /// For renames: 100 for identical contents, else the git-style similarity score.
    pub similarity: Option<u8>,
}

impl FileChange {
    /// The head path, or the base path for a deleted file.
    pub fn path(&self) -> &str {
        self.head
            .as_ref()
            .or(self.base.as_ref())
            .map_or("", |s| s.path.as_str())
    }
}

/// Renames below this similarity (percent) are reported as a deletion and an addition, as
/// git does by default.
pub const RENAME_THRESHOLD: u8 = 50;
/// Beyond this many deleted × added candidate pairs, only exact renames are detected.
pub const RENAME_PAIR_LIMIT: usize = 10_000;
/// The edit-distance limit used when scoring rename candidates.
const RENAME_DIFF_COST: usize = 500;

/// The files that differ between two trees, sorted by path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeChanges {
    pub files: Vec<FileChange>,
    /// Similarity renames were skipped because there were too many candidates.
    pub renames_limited: bool,
}

/// Several merge bases and the one chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeBase {
    /// Full commit id of the chosen base.
    pub chosen: String,
    /// Every merge base, sorted by commit id. More than one for criss-cross histories.
    pub all: Vec<String>,
}

/// A repository opened for reading objects.
pub struct ObjectStore {
    repo: gix::Repository,
}

impl ObjectStore {
    /// Opens the repository whose top level is `root`.
    pub fn open(root: &Path) -> Result<Self, String> {
        let repo = gix::open(root).map_err(|e| format!("cannot open the repository: {e}"))?;
        Ok(Self { repo })
    }

    /// Opens the repository that contains `path` (any folder or file inside its working
    /// folder), as `git` discovers it.
    pub fn discover(path: &Path) -> Result<Self, String> {
        let repo = gix::discover(path).map_err(|e| format!("not in a git repository: {e}"))?;
        Ok(Self { repo })
    }

    /// The working folder of this repository, `None` for a bare one.
    pub fn workdir(&self) -> Option<PathBuf> {
        self.repo.workdir().map(Path::to_path_buf)
    }

    /// The full id of the commit `rev` names.
    pub fn commit_id(&self, rev: &str) -> Result<String, String> {
        Ok(self.commit(rev)?.to_string())
    }

    fn commit(&self, rev: &str) -> Result<ObjectId, String> {
        let id = self
            .repo
            .rev_parse_single(rev)
            .map_err(|e| format!("`{rev}` is not in this repository: {e}"))?;
        let commit = id
            .object()
            .map_err(|e| e.to_string())?
            .peel_to_commit()
            .map_err(|e| format!("`{rev}` is not a commit: {e}"))?;
        Ok(commit.id)
    }

    fn tree_of(&self, commit: ObjectId) -> Result<ObjectId, String> {
        let c = self.repo.find_commit(commit).map_err(|e| e.to_string())?;
        c.tree_id().map(|t| t.detach()).map_err(|e| e.to_string())
    }

    /// The merge bases of `a` and `b`. With several, the chosen one is the most recently
    /// committed, ties broken by the smallest commit id. `None` when the histories are
    /// unrelated.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<Option<MergeBase>, String> {
        let (a, b) = (self.commit(a)?, self.commit(b)?);
        let mut all: Vec<ObjectId> = self
            .repo
            .merge_bases_many(a, &[b])
            .map_err(|e| format!("cannot compute the merge base: {e}"))?
            .into_iter()
            .map(|id| id.detach())
            .collect();
        all.sort();
        all.dedup();
        let mut best: Option<(i64, ObjectId)> = None;
        for id in &all {
            let commit = self.repo.find_commit(*id).map_err(|e| e.to_string())?;
            let time = commit.time().map_err(|e| e.to_string())?.seconds;
            if best.is_none_or(|(t, _)| time > t) {
                best = Some((time, *id));
            }
        }
        Ok(best.map(|(_, chosen)| MergeBase {
            chosen: chosen.to_string(),
            all: all.iter().map(ToString::to_string).collect(),
        }))
    }

    /// The bytes of the file at `path` (repository-relative, `/`-separated) in the commit
    /// `rev` names; `None` when the commit has no such file.
    pub fn blob_at(&self, rev: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
        let commit = self
            .repo
            .find_commit(self.commit(rev)?)
            .map_err(|e| e.to_string())?;
        let tree = commit.tree().map_err(|e| e.to_string())?;
        let Some(entry) = tree.lookup_entry_by_path(path).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let object = entry.object().map_err(|e| e.to_string())?;
        Ok((object.kind == gix::object::Kind::Blob).then(|| object.detach().data))
    }

    /// A blob's bytes.
    pub fn blob(&self, id: ObjectId) -> Result<Vec<u8>, String> {
        let object = self.repo.find_object(id).map_err(|e| e.to_string())?;
        if object.kind != gix::object::Kind::Blob {
            return Err(format!("{} is not a blob", short(&id.to_string())));
        }
        Ok(object.detach().data)
    }

    /// The files that differ between commits `base` and `head` (full ids or any revision).
    pub fn changed_files(&self, base: &str, head: &str) -> Result<TreeChanges, String> {
        let (b, h) = (self.commit(base)?, self.commit(head)?);
        let (bt, ht) = (self.tree_of(b)?, self.tree_of(h)?);
        let mut deleted = vec![];
        let mut added = vec![];
        let mut modified = vec![];
        self.walk(
            Some(bt),
            Some(ht),
            "",
            &mut deleted,
            &mut added,
            &mut modified,
        )?;
        let mut out = TreeChanges::default();
        out.files.extend(modified);
        self.pair_renames(deleted, added, &mut out)?;
        out.files
            .sort_by(|x, y| x.path().cmp(y.path()).then(x.status.cmp(&y.status)));
        Ok(out)
    }

    fn entries(&self, tree: Option<ObjectId>) -> Result<BTreeMap<Vec<u8>, Entry>, String> {
        let mut out = BTreeMap::new();
        let Some(tree) = tree else {
            return Ok(out);
        };
        let t = self.repo.find_tree(tree).map_err(|e| e.to_string())?;
        let decoded = t.decode().map_err(|e| e.to_string())?;
        for e in decoded.entries {
            let mode = e.mode;
            let kind = if mode.is_tree() {
                None
            } else if mode.is_commit() {
                Some(EntryKind::Submodule)
            } else if mode.is_link() {
                Some(EntryKind::Symlink)
            } else {
                Some(EntryKind::File)
            };
            out.insert(
                e.filename.to_vec(),
                Entry {
                    id: e.oid.to_owned(),
                    kind,
                    executable: mode.is_executable(),
                },
            );
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn walk(
        &self,
        base: Option<ObjectId>,
        head: Option<ObjectId>,
        prefix: &str,
        deleted: &mut Vec<FileSide>,
        added: &mut Vec<FileSide>,
        modified: &mut Vec<FileChange>,
    ) -> Result<(), String> {
        if base == head {
            return Ok(());
        }
        let (be, he) = (self.entries(base)?, self.entries(head)?);
        let mut names: Vec<&Vec<u8>> = be.keys().chain(he.keys()).collect();
        names.sort();
        names.dedup();
        for name in names {
            let path = format!("{prefix}{}", String::from_utf8_lossy(name));
            let (b, h) = (be.get(name), he.get(name));
            let side = |e: &Entry| {
                e.kind.map(|kind| FileSide {
                    path: path.clone(),
                    id: e.id,
                    kind,
                    executable: e.executable,
                })
            };
            let (bf, hf) = (b.and_then(side), h.and_then(side));
            let bt = b.filter(|e| e.kind.is_none()).map(|e| e.id);
            let ht = h.filter(|e| e.kind.is_none()).map(|e| e.id);
            match (bf, hf) {
                (Some(x), Some(y)) => {
                    if x.id != y.id || x.kind != y.kind || x.executable != y.executable {
                        modified.push(FileChange {
                            status: FileStatus::Modified,
                            base: Some(x),
                            head: Some(y),
                            similarity: None,
                        });
                    }
                }
                (Some(x), None) => deleted.push(x),
                (None, Some(y)) => added.push(y),
                (None, None) => {}
            }
            if bt.is_some() || ht.is_some() {
                self.walk(bt, ht, &format!("{path}/"), deleted, added, modified)?;
            }
        }
        Ok(())
    }

    /// Pairs deletions with additions: identical contents first, then the most similar
    /// pairs at or above [`RENAME_THRESHOLD`]. Unpaired ones stay deleted or added.
    fn pair_renames(
        &self,
        deleted: Vec<FileSide>,
        added: Vec<FileSide>,
        out: &mut TreeChanges,
    ) -> Result<(), String> {
        let mut del: Vec<Option<FileSide>> = deleted.into_iter().map(Some).collect();
        let mut add: Vec<Option<FileSide>> = added.into_iter().map(Some).collect();
        let pair = |out: &mut TreeChanges, b: FileSide, h: FileSide, score: u8| {
            out.files.push(FileChange {
                status: FileStatus::Renamed,
                base: Some(b),
                head: Some(h),
                similarity: Some(score),
            });
        };
        // Exact: both lists are in path order, so equal ids pair in path order.
        for slot in del.iter_mut() {
            let Some(d) = slot.as_ref() else { continue };
            if let Some(j) = add
                .iter()
                .position(|a| a.as_ref().is_some_and(|a| a.id == d.id && a.kind == d.kind))
            {
                let (b, h) = (slot.take().unwrap(), add[j].take().unwrap());
                pair(out, b, h, 100);
            }
        }
        // Similar: only regular files, scored by shared lines.
        let files = |v: &[Option<FileSide>]| -> Vec<usize> {
            (0..v.len())
                .filter(|&i| v[i].as_ref().is_some_and(|s| s.kind == EntryKind::File))
                .collect()
        };
        let (di, ai) = (files(&del), files(&add));
        if !di.is_empty() && !ai.is_empty() {
            if di.len() * ai.len() > RENAME_PAIR_LIMIT {
                out.renames_limited = true;
            } else {
                let read = |v: &[Option<FileSide>], ix: &[usize]| -> Result<Vec<Vec<u8>>, String> {
                    ix.iter()
                        .map(|&i| self.blob(v[i].as_ref().unwrap().id))
                        .collect()
                };
                let (dblobs, ablobs) = (read(&del, &di)?, read(&add, &ai)?);
                let mut scored = vec![];
                for (p, db) in dblobs.iter().enumerate() {
                    for (q, ab) in ablobs.iter().enumerate() {
                        let (small, large) = (db.len().min(ab.len()), db.len().max(ab.len()));
                        if is_binary(db) || is_binary(ab) || small * 100 < large * 50 {
                            continue;
                        }
                        let s = similarity(db, ab, RENAME_DIFF_COST);
                        if s >= RENAME_THRESHOLD {
                            scored.push((std::cmp::Reverse(s), di[p], ai[q]));
                        }
                    }
                }
                scored.sort();
                for (std::cmp::Reverse(s), i, j) in scored {
                    if del[i].is_some() && add[j].is_some() {
                        let (b, h) = (del[i].take().unwrap(), add[j].take().unwrap());
                        pair(out, b, h, s);
                    }
                }
            }
        }
        for b in del.into_iter().flatten() {
            out.files.push(FileChange {
                status: FileStatus::Deleted,
                base: Some(b),
                head: None,
                similarity: None,
            });
        }
        for h in add.into_iter().flatten() {
            out.files.push(FileChange {
                status: FileStatus::Added,
                base: None,
                head: Some(h),
                similarity: None,
            });
        }
        Ok(())
    }
}

struct Entry {
    id: ObjectId,
    /// `None` for a subtree.
    kind: Option<EntryKind>,
    executable: bool,
}

/// Git's test: a NUL byte in the first 8000 bytes (the one the viewer uses too).
pub use cho_core::is_binary;
