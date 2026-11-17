//! The changed files and changed declarations between two commits of a local repository.
//!
//! Files come from a tree compare in the object store ([`cho_git::ObjectStore`]); changed
//! lines from an in-process line diff ([`cho_git::linediff`]). Each side's hunks are mapped
//! onto that side's Tree-sitter declarations ([`cho_syntax::outline`]): head-side hunks give
//! `added` and `changed` declarations, base-side hunks give `removed` (and `changed`) ones.
//! Declarations are matched across sides by qualified name and kind only; a name that has
//! several declarations on either side is reported as `ambiguous`, never resolved. Nothing
//! reads or moves the checkout. See `docs/public/cli.md` ("Changes output").

mod split;

pub use split::{Piece, split_hunks};

use cho_git::linediff::{DEFAULT_MAX_COST, Lines, diff};
pub use cho_git::{EntryKind, FileStatus};
use cho_git::{FileChange, FileSide, ObjectStore, TreeChanges, is_binary, short};
use cho_syntax::{Language, Symbol, SymbolKind, outline};
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// Version of the JSON shape; bumped only for incompatible changes.
pub const FORMAT_VERSION: u32 = 1;

/// Everything that differs between two commits.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChangeSet {
    pub version: u32,
    /// Full commit id of Before.
    pub base: String,
    /// Full commit id of After.
    pub head: String,
    /// Present when `base` was computed as the merge base of `head` and a target.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_base: Option<MergeBaseInfo>,
    /// After is the working folder, not a commit: `head` is the checked-out commit, and each
    /// hunk and declaration says whether uncommitted work touches it.
    #[serde(skip_serializing_if = "is_false")]
    pub worktree: bool,
    /// Sorted by path.
    pub files: Vec<FileChanges>,
    /// Facts the reader should know (several merge bases, limits that applied).
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MergeBaseInfo {
    /// The target revision as given.
    pub target: String,
    /// Every merge base (full ids, sorted). `base` is one of them.
    pub candidates: Vec<String>,
}

/// One changed file.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileChanges {
    /// `added`, `modified`, `deleted` or `renamed`.
    #[serde(serialize_with = "as_str::file_status")]
    pub status: FileStatus,
    /// The head path, or the base path for a deleted file.
    pub path: String,
    /// The base path of a renamed file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    /// For renames: 100 for identical contents, else the share of Before's bytes kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity: Option<u8>,
    /// `file`, `symlink` or `submodule` (the head side's, else the base side's).
    #[serde(serialize_with = "as_str::entry_kind")]
    pub kind: EntryKind,
    /// The language declarations were read with (head side's, else base side's), if any.
    #[serde(serialize_with = "as_str::language")]
    pub language: Option<Language>,
    /// Whether declarations were compared; written as `level` (`symbol` or `file`) and, at
    /// file level, `reason`.
    #[serde(flatten)]
    pub level: Level,
    /// Some changed lines are outside every declaration (imports, top-level statements).
    pub outside_declarations: bool,
    /// The line diff hit its cost limit; hunks are one block per file, not minimal.
    pub coarse: bool,
    /// The file differs between HEAD and the working folder (only in a worktree comparison;
    /// an untracked file counts).
    #[serde(skip_serializing_if = "is_false")]
    pub uncommitted: bool,
    pub hunks: Vec<Hunk>,
    pub symbols: Vec<SymbolChange>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// How far a file was compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Declarations were compared.
    Symbol,
    /// Only the file's lines were compared, for this reason.
    File(FileOnly),
}

/// Why a file is compared at file level only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOnly {
    UnsupportedLanguage,
    Binary,
    NotUtf8,
    Symlink,
    Submodule,
    /// The working file is over Chorong's source size limit.
    TooLarge,
    /// The working file exists but can't be read as a file inside the folder (permissions,
    /// a symbolic link out of the folder, a directory).
    Unreadable,
}

impl FileOnly {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedLanguage => "unsupported-language",
            Self::Binary => "binary",
            Self::NotUtf8 => "not-utf8",
            Self::Symlink => "symlink",
            Self::Submodule => "submodule",
            Self::TooLarge => "too-large",
            Self::Unreadable => "unreadable",
        }
    }
}

impl Level {
    /// Why the file is at file level only; `None` at symbol level.
    pub fn reason(self) -> Option<FileOnly> {
        match self {
            Self::Symbol => None,
            Self::File(r) => Some(r),
        }
    }
}

impl Serialize for Level {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Json {
            level: &'static str,
            #[serde(skip_serializing_if = "Option::is_none")]
            reason: Option<&'static str>,
        }
        Json {
            level: match self {
                Self::Symbol => "symbol",
                Self::File(_) => "file",
            },
            reason: self.reason().map(FileOnly::as_str),
        }
        .serialize(s)
    }
}

/// How a declaration changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SymbolStatus {
    /// Not at base.
    Added,
    Changed,
    /// Not at head.
    Removed,
    /// Several declarations with this name and kind on a side; all are listed.
    Ambiguous,
}

impl SymbolStatus {
    /// The name the JSON uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Changed => "changed",
            Self::Removed => "removed",
            Self::Ambiguous => "ambiguous",
        }
    }
}

/// Serialisers that write the other crates' enums as their `as_str` names.
mod as_str {
    use super::*;

    pub fn file_status<S: Serializer>(v: &FileStatus, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(v.as_str())
    }

    pub fn entry_kind<S: Serializer>(v: &EntryKind, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(v.as_str())
    }

    pub fn symbol_kind<S: Serializer>(v: &SymbolKind, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(v.as_str())
    }

    pub fn language<S: Serializer>(v: &Option<Language>, s: S) -> Result<S::Ok, S::Error> {
        v.map(Language::name).serialize(s)
    }
}

/// A changed region: `base` lines were replaced by `head` lines.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Hunk {
    pub base: Span,
    pub head: Span,
    /// HEAD to the working folder touches this hunk (only in a worktree comparison). A hunk
    /// that committed and uncommitted work both touch is cut at the edge of the uncommitted
    /// part, so each hunk is one or the other.
    #[serde(skip_serializing_if = "is_false")]
    pub uncommitted: bool,
}

/// Lines on one side. An empty span (`line_count` 0) is the position before `start_line`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Span {
    /// One-based.
    pub start_line: usize,
    pub line_count: usize,
    /// UTF-8 byte offsets, end exclusive.
    pub bytes: [usize; 2],
}

/// A changed declaration.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SymbolChange {
    /// As the language writes it: `Queue::push`, `Queue.push`.
    pub qualified_name: String,
    #[serde(serialize_with = "as_str::symbol_kind")]
    pub kind: SymbolKind,
    pub status: SymbolStatus,
    /// The declarations with this name and kind at base, in source order.
    pub base: Vec<Decl>,
    /// The declarations with this name and kind at head, in source order.
    pub head: Vec<Decl>,
    /// Uncommitted work touches it (only in a worktree comparison).
    #[serde(skip_serializing_if = "is_false")]
    pub uncommitted: bool,
}

/// One declaration on one side.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Decl {
    /// The whole declaration, UTF-8 byte offsets, end exclusive.
    pub range: [usize; 2],
    pub name_range: [usize; 2],
    /// One-based, inclusive.
    pub start_line: usize,
    pub end_line: usize,
    /// Changed lines on this side fall inside it (outside its nested declarations).
    pub touched: bool,
}

/// Which commit of the comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// Before.
    Base,
    /// After.
    Head,
}

/// The identity of a declaration on one side: repository-relative path, qualified name and
/// kind, as the `cho-syntax` outline gives them. The same declaration on the other side has
/// the same name and kind; its path differs only when the file was renamed (see
/// [`FileChanges::path_at`]). Several declarations can share a key (see `ambiguous`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct SymbolKey {
    pub path: String,
    pub qualified_name: String,
    #[serde(serialize_with = "as_str::symbol_kind")]
    pub kind: SymbolKind,
}

impl SymbolKey {
    /// The key of an outline declaration in the file at `path`.
    pub fn of(path: &str, symbol: &Symbol, language: Language) -> Self {
        Self {
            path: path.to_string(),
            qualified_name: symbol.qualified_name(language),
            kind: symbol.kind,
        }
    }
}

impl FileChanges {
    /// A file with no hunks or declarations yet, at symbol level until analysis says
    /// otherwise.
    fn new(status: FileStatus, path: &str, kind: EntryKind, language: Option<Language>) -> Self {
        Self {
            status,
            path: path.to_string(),
            old_path: None,
            similarity: None,
            kind,
            language,
            level: Level::Symbol,
            outside_declarations: false,
            coarse: false,
            uncommitted: false,
            hunks: vec![],
            symbols: vec![],
        }
    }

    fn file_only(mut self, reason: FileOnly) -> Self {
        self.level = Level::File(reason);
        self
    }

    /// The file's path on `side`: `old_path` at base for a rename; `None` on the side where
    /// the file does not exist (base of an added file, head of a deleted one).
    pub fn path_at(&self, side: Side) -> Option<&str> {
        match (side, self.status) {
            (Side::Base, FileStatus::Added) | (Side::Head, FileStatus::Deleted) => None,
            (Side::Base, _) => Some(self.old_path.as_deref().unwrap_or(&self.path)),
            (Side::Head, _) => Some(&self.path),
        }
    }

    /// The key of `symbol` (one of this file's) on `side`, `None` where the file does not exist.
    pub fn symbol_key(&self, symbol: &SymbolChange, side: Side) -> Option<SymbolKey> {
        Some(SymbolKey {
            path: self.path_at(side)?.to_string(),
            qualified_name: symbol.qualified_name.clone(),
            kind: symbol.kind,
        })
    }
}

/// Changes between `base` and `head` (any revisions naming commits) in the repository at
/// `root`.
pub fn compare(root: &Path, base: &str, head: &str) -> Result<ChangeSet, String> {
    let store = ObjectStore::open(root)?;
    let (b, h) = (store.commit_id(base)?, store.commit_id(head)?);
    compare_ids(&store, b, h, None, vec![])
}

/// Changes from the merge base of `head` and `target` to `head`.
pub fn compare_with_merge_base(root: &Path, target: &str, head: &str) -> Result<ChangeSet, String> {
    let store = ObjectStore::open(root)?;
    let h = store.commit_id(head)?;
    let (base, info, notes) = merge_base_of(&store, target, &h)?;
    compare_ids(&store, base, h, Some(info), notes)
}

fn compare_ids(
    store: &ObjectStore,
    base: String,
    head: String,
    merge_base: Option<MergeBaseInfo>,
    mut notes: Vec<String>,
) -> Result<ChangeSet, String> {
    let tree = store.changed_files(&base, &head)?;
    notes.extend(rename_note(&tree));
    let files = tree
        .files
        .iter()
        .map(|f| file_changes(store, f))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ChangeSet {
        version: FORMAT_VERSION,
        base,
        head,
        merge_base,
        worktree: false,
        files,
        notes,
    })
}

/// The note for a tree compare whose rename search was cut short.
fn rename_note(tree: &TreeChanges) -> Option<String> {
    tree.renames_limited.then(|| {
        format!(
            "Too many added and deleted files to look for edited renames; only renames with identical contents are shown (limit {} pairs).",
            cho_git::RENAME_PAIR_LIMIT
        )
    })
}

/// The repository's working folder.
fn workdir(store: &ObjectStore) -> Result<PathBuf, String> {
    store
        .workdir()
        .ok_or_else(|| "this repository has no working folder".to_string())
}

/// Why a file is at file level only because a side is not a regular file (a symlink or a
/// submodule); `None` when both sides are files.
fn non_file_reason(f: &FileChange) -> Option<FileOnly> {
    [f.base.as_ref(), f.head.as_ref()]
        .into_iter()
        .flatten()
        .find_map(|s| match s.kind {
            EntryKind::File => None,
            EntryKind::Symlink => Some(FileOnly::Symlink),
            EntryKind::Submodule => Some(FileOnly::Submodule),
        })
}

/// One side's contents, ready for declarations.
struct SideText {
    bytes: Vec<u8>,
    language: Option<Language>,
    decls: Vec<Symbol>,
}

fn file_changes(store: &ObjectStore, f: &FileChange) -> Result<FileChanges, String> {
    let main = f.head.as_ref().or(f.base.as_ref()).expect("a side");
    let lang_of = |s: &FileSide| structural_language(&s.path);
    let language = f
        .head
        .as_ref()
        .and_then(lang_of)
        .or_else(|| f.base.as_ref().and_then(lang_of));
    let mut out = FileChanges::new(f.status, f.path(), main.kind, language);
    out.old_path = (f.status == FileStatus::Renamed)
        .then(|| f.base.as_ref().map(|b| b.path.clone()))
        .flatten();
    out.similarity = f.similarity;
    if let Some(reason) = non_file_reason(f) {
        return Ok(out.file_only(reason));
    }
    let read = |s: Option<&FileSide>| -> Result<Vec<u8>, String> {
        s.map_or(Ok(vec![]), |s| store.blob(s.id))
    };
    let (bb, hb) = (read(f.base.as_ref())?, read(f.head.as_ref())?);
    Ok(analyze(
        out,
        f.base.as_ref().map(|s| s.path.as_str()),
        f.head.as_ref().map(|s| s.path.as_str()),
        &bb,
        &hb,
        None,
    ))
}

/// The language whose declarations are compared for `path`: none for unsupported files and for
/// the highlight-only formats (JSON, TOML, XML, Markdown), which stay at file level.
fn structural_language(path: &str) -> Option<Language> {
    Language::from_path(Path::new(path)).filter(|l| l.has_structure())
}

/// Diffs the two sides' bytes into hunks and declaration changes (only the file level when a
/// side is binary). With `head_bytes` (a worktree comparison: `hb` is the working folder,
/// `head_bytes` the checked-out commit's file) hunks are cut by what is committed and flagged
/// `uncommitted`.
fn analyze(
    mut out: FileChanges,
    base_path: Option<&str>,
    head_path: Option<&str>,
    bb: &[u8],
    hb: &[u8],
    head_bytes: Option<&[u8]>,
) -> FileChanges {
    if is_binary(bb) || is_binary(hb) {
        return out.file_only(FileOnly::Binary);
    }
    let language = head_path.or(base_path).and_then(structural_language);
    let (bl, hl) = (Lines::new(bb), Lines::new(hb));
    let d = diff(&bl, &hl, DEFAULT_MAX_COST);
    out.coarse = d.coarse;
    let pieces: Vec<Piece> = match head_bytes {
        None => d
            .hunks
            .iter()
            .map(|h| Piece {
                base: h.base.clone(),
                head: h.head.clone(),
                uncommitted: false,
            })
            .collect(),
        Some(head) => {
            let hd = Lines::new(head);
            let (committed, uncommitted) = (
                diff(&bl, &hd, DEFAULT_MAX_COST),
                diff(&hd, &hl, DEFAULT_MAX_COST),
            );
            out.coarse |= committed.coarse || uncommitted.coarse;
            split_hunks(&d.hunks, &committed.hunks, &uncommitted.hunks)
        }
    };
    out.hunks = pieces
        .iter()
        .map(|p| Hunk {
            base: span(&bl, &p.base),
            head: span(&hl, &p.head),
            uncommitted: p.uncommitted,
        })
        .collect();
    if language.is_none() {
        return out.file_only(FileOnly::UnsupportedLanguage);
    }
    let (Ok(bs), Ok(hs)) = (std::str::from_utf8(bb), std::str::from_utf8(hb)) else {
        return out.file_only(FileOnly::NotUtf8);
    };
    out.level = Level::Symbol;
    let side = |path: Option<&str>, text: &str| {
        let language = path.and_then(structural_language);
        SideText {
            bytes: text.as_bytes().to_vec(),
            language,
            decls: language.map_or(vec![], |l| outline(l, text)),
        }
    };
    let (base, head) = (side(base_path, bs), side(head_path, hs));
    let spans = |pieces: &[&Piece]| -> (Vec<Range<usize>>, Vec<Range<usize>>) {
        (
            pieces.iter().map(|p| bl.bytes(&p.base)).collect(),
            pieces.iter().map(|p| hl.bytes(&p.head)).collect(),
        )
    };
    let all: Vec<&Piece> = pieces.iter().collect();
    let unc: Vec<&Piece> = pieces.iter().filter(|p| p.uncommitted).collect();
    let (base_spans, head_spans) = spans(&all);
    let (ubase_spans, uhead_spans) = spans(&unc);
    let (bt, b_out) = touched(&base, &base_spans);
    let (ht, h_out) = touched(&head, &head_spans);
    let (ubt, _) = touched(&base, &ubase_spans);
    let (uht, _) = touched(&head, &uhead_spans);
    out.outside_declarations = b_out || h_out;
    out.symbols = classify(&base, (&bt, &ubt), &bl, &head, (&ht, &uht), &hl);
    out
}

fn span(lines: &Lines<'_>, r: &Range<usize>) -> Span {
    let b = lines.bytes(r);
    Span {
        start_line: r.start + 1,
        line_count: r.len(),
        bytes: [b.start, b.end],
    }
}

/// The bytes a declaration owns for change detection: its range, extended up over the lines
/// directly above it that are comments, doc comments, attributes or decorators.
fn owned_range(src: &[u8], d: &Symbol) -> Range<usize> {
    let mut start = line_start(src, d.range.start);
    if has_text(src, start..d.range.start) {
        return d.range.clone();
    }
    while start > 0 {
        let prev = line_start(src, start - 1);
        let line = String::from_utf8_lossy(&src[prev..start]);
        let t = line.trim();
        let leading = ["//", "/*", "*", "#", "@"].iter().any(|p| t.starts_with(p));
        if t.is_empty() || !leading {
            break;
        }
        start = prev;
    }
    start..d.range.end
}

/// `r` widened to whole lines.
fn whole_lines(src: &[u8], r: &Range<usize>) -> Range<usize> {
    let end = src[r.end..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(src.len(), |i| r.end + i + 1);
    line_start(src, r.start)..end
}

fn line_start(src: &[u8], at: usize) -> usize {
    src[..at]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1)
}

fn has_text(src: &[u8], r: Range<usize>) -> bool {
    src[r].iter().any(|b| !b.is_ascii_whitespace())
}

/// Which declarations the changed spans touch, and whether any changed text lies outside
/// every declaration. A span touches a declaration when it covers non-whitespace bytes the
/// declaration owns outside its nested declarations. Empty spans (lines only added or only
/// removed) touch nothing on their side: the other side's span says what changed, so adding
/// a method does not mark its class as changed.
fn touched(side: &SideText, spans: &[Range<usize>]) -> (Vec<bool>, bool) {
    let src = &side.bytes;
    let owned: Vec<Range<usize>> = side.decls.iter().map(|d| owned_range(src, d)).collect();
    let nested: Vec<Vec<Range<usize>>> = owned
        .iter()
        .map(|r| {
            owned
                .iter()
                .filter(|c| r.start <= c.start && c.end <= r.end && *c != r)
                .cloned()
                .collect()
        })
        .collect();
    // Text on a declaration's own lines (`export` before `function`) is not "outside".
    let decl_lines: Vec<Range<usize>> = owned.iter().map(|r| whole_lines(src, r)).collect();
    let mut hit = vec![false; owned.len()];
    let mut outside = false;
    for s in spans {
        for i in 0..owned.len() {
            let r = &owned[i];
            let (lo, hi) = (r.start.max(s.start), r.end.min(s.end));
            if lo < hi && uncovered_text(src, lo..hi, &nested[i]) {
                hit[i] = true;
            }
        }
        if uncovered_text(src, s.clone(), &decl_lines) {
            outside = true;
        }
    }
    (hit, outside)
}

/// Whether `r` has non-whitespace bytes outside every range in `holes`.
fn uncovered_text(src: &[u8], r: Range<usize>, holes: &[Range<usize>]) -> bool {
    let mut holes: Vec<Range<usize>> = holes
        .iter()
        .filter(|h| h.start < r.end && r.start < h.end)
        .cloned()
        .collect();
    holes.sort_by_key(|h| (h.start, h.end));
    let mut at = r.start;
    for h in holes {
        if h.start > at && has_text(src, at..h.start.min(r.end)) {
            return true;
        }
        at = at.max(h.end);
        if at >= r.end {
            return false;
        }
    }
    at < r.end && has_text(src, at..r.end)
}

type Key = (String, SymbolKind);

/// Declarations touched by the changes, and by the uncommitted ones.
type Touched<'a> = (&'a [bool], &'a [bool]);

fn classify(
    base: &SideText,
    (bt, ubt): Touched<'_>,
    bl: &Lines<'_>,
    head: &SideText,
    (ht, uht): Touched<'_>,
    hl: &Lines<'_>,
) -> Vec<SymbolChange> {
    let index = |side: &SideText| -> BTreeMap<Key, Vec<usize>> {
        let mut m: BTreeMap<Key, Vec<usize>> = BTreeMap::new();
        if let Some(l) = side.language {
            for (i, d) in side.decls.iter().enumerate() {
                m.entry((d.qualified_name(l), d.kind)).or_default().push(i);
            }
        }
        m
    };
    let (bi, hi) = (index(base), index(head));
    let mut keys: Vec<&Key> = bi
        .iter()
        .filter(|(_, v)| v.iter().any(|&i| bt[i]))
        .chain(hi.iter().filter(|(_, v)| v.iter().any(|&i| ht[i])))
        .map(|(k, _)| k)
        .collect();
    keys.sort();
    keys.dedup();
    let decls = |side: &SideText, touched: &[bool], lines: &Lines<'_>, ix: Option<&Vec<usize>>| {
        ix.map_or(vec![], |ix| {
            ix.iter()
                .map(|&i| {
                    let d = &side.decls[i];
                    Decl {
                        range: [d.range.start, d.range.end],
                        name_range: [d.name_range.start, d.name_range.end],
                        start_line: line_of(lines, d.range.start),
                        end_line: line_of(lines, d.range.end.saturating_sub(1).max(d.range.start)),
                        touched: touched[i],
                    }
                })
                .collect()
        })
    };
    let mut out: Vec<SymbolChange> = keys
        .into_iter()
        .map(|k| {
            let b = decls(base, bt, bl, bi.get(k));
            let h = decls(head, ht, hl, hi.get(k));
            let status = if b.len() > 1 || h.len() > 1 {
                SymbolStatus::Ambiguous
            } else if b.is_empty() {
                SymbolStatus::Added
            } else if h.is_empty() {
                SymbolStatus::Removed
            } else {
                SymbolStatus::Changed
            };
            SymbolChange {
                qualified_name: k.0.clone(),
                kind: k.1,
                status,
                base: b,
                head: h,
                uncommitted: bi.get(k).is_some_and(|v| v.iter().any(|&i| ubt[i]))
                    || hi.get(k).is_some_and(|v| v.iter().any(|&i| uht[i])),
            }
        })
        .collect();
    out.sort_by_key(|s| {
        (
            s.head.first().map_or(usize::MAX, |d| d.range[0]),
            s.base.first().map_or(usize::MAX, |d| d.range[0]),
            s.qualified_name.clone(),
            s.kind.as_str(),
        )
    });
    out
}

/// One-based line of byte `at`.
fn line_of(lines: &Lines<'_>, at: usize) -> usize {
    let (mut lo, mut hi) = (0, lines.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if lines.offset(mid) <= at {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo.max(1)
}

// ---- The working folder as After ----

/// Changes from `base` (any revision naming a commit) to the working folder of the repository
/// whose top level is `root`. `head` is the checked-out commit; files come from the tree
/// compare base to HEAD plus the files that differ between HEAD and the working folder
/// (untracked ones count as added, gitignored ones are skipped), and every hunk and
/// declaration says whether uncommitted work touches it. No git subprocess; nothing is
/// written.
pub fn compare_worktree(root: &Path, base: &str) -> Result<ChangeSet, String> {
    let store = ObjectStore::open(root)?;
    let (b, h) = (store.commit_id(base)?, store.commit_id("HEAD")?);
    worktree_ids(&store, b, h, None, vec![])
}

/// [`compare_worktree`] from the merge base of HEAD and `target`.
pub fn compare_worktree_with_merge_base(root: &Path, target: &str) -> Result<ChangeSet, String> {
    let store = ObjectStore::open(root)?;
    let h = store.commit_id("HEAD")?;
    let (base, info, notes) = merge_base_of(&store, target, &h)?;
    worktree_ids(&store, base, h, Some(info), notes)
}

/// The changes between two versions of one file at `path`, with no repository: what
/// [`compare`] reports for a modified file. With `committed` (the HEAD version, `head`
/// being the working folder's), hunks and declarations say whether uncommitted work
/// touches them, as in [`compare_worktree`]. For tests and fuzzing.
pub fn compare_texts(
    path: &str,
    base: &[u8],
    head: &[u8],
    committed: Option<&[u8]>,
) -> FileChanges {
    let language = structural_language(path);
    let mut out = FileChanges::new(FileStatus::Modified, path, EntryKind::File, language);
    out.uncommitted = committed.is_some_and(|c| c != head);
    analyze(out, Some(path), Some(path), base, head, committed)
}

fn merge_base_of(
    store: &ObjectStore,
    target: &str,
    head: &str,
) -> Result<(String, MergeBaseInfo, Vec<String>), String> {
    let mb = store
        .merge_base(target, head)?
        .ok_or_else(|| format!("`{target}` and `{head}` have no common history"))?;
    let mut notes = vec![];
    if mb.all.len() > 1 {
        notes.push(format!(
            "{} merge bases ({}); Before is {}, the most recently committed.",
            mb.all.len(),
            mb.all
                .iter()
                .map(|c| short(c))
                .collect::<Vec<_>>()
                .join(", "),
            short(&mb.chosen)
        ));
    }
    let info = MergeBaseInfo {
        target: target.to_string(),
        candidates: mb.all,
    };
    Ok((mb.chosen, info, notes))
}

/// The bytes of a working-folder file, `None` when it is not a readable file.
/// A file in the working folder: gone, read, or there but not readable as source.
enum Work {
    Missing,
    Bytes(Vec<u8>),
    Unreadable(FileOnly),
}

fn read_work(workdir: &Path, path: &str) -> Work {
    use cho_core::fs::ReadError;
    match cho_core::fs::read_source_in(workdir, Path::new(path)) {
        Ok(b) => Work::Bytes(b),
        Err(ReadError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Work::Missing,
        Err(ReadError::TooLarge { .. }) => Work::Unreadable(FileOnly::TooLarge),
        Err(_) => Work::Unreadable(FileOnly::Unreadable),
    }
}

/// Whether `path` differs between HEAD and the working folder (a file only on one side does).
/// A file Chorong can't read counts as differing: git already listed it, and hiding it would
/// hide a change.
fn differs_from_head(store: &ObjectStore, workdir: &Path, path: &str) -> bool {
    let head = store.blob_at("HEAD", path).ok().flatten();
    match read_work(workdir, path) {
        Work::Bytes(w) => head.as_ref() != Some(&w),
        Work::Missing => head.is_some(),
        Work::Unreadable(_) => true,
    }
}

/// The files that differ between HEAD and the working folder, sorted: edited, staged, deleted
/// and untracked ones, not gitignored ones. A file touched without a change is not listed.
pub fn uncommitted_files(root: &Path) -> Result<Vec<String>, String> {
    let store = ObjectStore::open(root)?;
    let workdir = workdir(&store)?;
    Ok(cho_git::uncommitted_paths(root)?
        .into_iter()
        .filter(|p| differs_from_head(&store, &workdir, p))
        .collect())
}

fn worktree_ids(
    store: &ObjectStore,
    base: String,
    head: String,
    merge_base: Option<MergeBaseInfo>,
    mut notes: Vec<String>,
) -> Result<ChangeSet, String> {
    let workdir = workdir(store)?;
    let tree = store.changed_files(&base, &head)?;
    notes.extend(rename_note(&tree));
    let dirty: std::collections::BTreeSet<String> = cho_git::uncommitted_paths(&workdir)?
        .into_iter()
        .filter(|p| differs_from_head(store, &workdir, p))
        .collect();
    let mut files = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for f in &tree.files {
        seen.insert(f.path().to_string());
        if dirty.contains(f.path()) {
            files.extend(worktree_file(store, &workdir, &base, Some(f), f.path())?);
        } else {
            files.push(file_changes(store, f)?);
        }
    }
    for path in dirty.iter().filter(|p| !seen.contains(*p)) {
        files.extend(worktree_file(store, &workdir, &base, None, path)?);
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ChangeSet {
        version: FORMAT_VERSION,
        base,
        head,
        merge_base,
        worktree: true,
        files,
        notes,
    })
}

/// The changes to one file (head path `path`, repository-relative) from `base` to the working
/// folder of the repository containing `root`, hunks cut by what is committed. `None` when the
/// file is the same at the base and in the working folder. For the viewer's gutter.
pub fn worktree_file_changes(
    root: &Path,
    base: &str,
    path: &str,
) -> Result<Option<FileChanges>, String> {
    let store = ObjectStore::discover(root)?;
    let workdir = workdir(&store)?;
    let tree = store.changed_files(base, "HEAD")?;
    let fc = tree
        .files
        .iter()
        .find(|f| f.head.as_ref().is_some_and(|h| h.path == path));
    if differs_from_head(&store, &workdir, path) {
        worktree_file(&store, &workdir, base, fc, path)
    } else {
        fc.map(|f| file_changes(&store, f)).transpose()
    }
}

/// One file that differs from HEAD in the working folder: `fc` is its base-to-HEAD change when
/// it has one. `None` when the working file equals the base file.
fn worktree_file(
    store: &ObjectStore,
    workdir: &Path,
    base: &str,
    fc: Option<&FileChange>,
    path: &str,
) -> Result<Option<FileChanges>, String> {
    if let Some(f) = fc.filter(|f| non_file_reason(f).is_some()) {
        return file_changes(store, f).map(Some);
    }
    let base_side: Option<(String, Vec<u8>)> = match fc {
        Some(f) => f
            .base
            .as_ref()
            .map(|s| Ok::<_, String>((s.path.clone(), store.blob(s.id)?)))
            .transpose()?,
        None => store.blob_at(base, path)?.map(|b| (path.to_string(), b)),
    };
    let head_bytes = store.blob_at("HEAD", path)?.unwrap_or_default();
    let (work, unreadable) = match read_work(workdir, path) {
        Work::Bytes(b) => (Some(b), None),
        Work::Missing => (None, None),
        // There but not readable: changed at file level, never "deleted".
        Work::Unreadable(reason) => (Some(vec![]), Some(reason)),
    };
    let renamed = base_side.as_ref().is_some_and(|(p, _)| p != path);
    let status = match (&base_side, &work) {
        (None, Some(_)) => FileStatus::Added,
        (Some(_), None) => FileStatus::Deleted,
        (Some((_, b)), Some(w)) => {
            if b == w && !renamed && unreadable.is_none() {
                return Ok(None);
            }
            if renamed {
                FileStatus::Renamed
            } else {
                FileStatus::Modified
            }
        }
        (None, None) => return Ok(None),
    };
    let language = structural_language(path);
    let mut out = FileChanges::new(status, path, EntryKind::File, language);
    out.old_path = renamed
        .then(|| base_side.as_ref().map(|(p, _)| p.clone()))
        .flatten();
    out.similarity = fc.and_then(|f| f.similarity).filter(|_| renamed);
    out.uncommitted = true;
    if let Some(reason) = unreadable {
        return Ok(Some(out.file_only(reason)));
    }
    let bb = base_side.as_ref().map_or(&[][..], |(_, b)| b.as_slice());
    let wb = work.as_deref().unwrap_or(&[]);
    Ok(Some(analyze(
        out,
        base_side.as_ref().map(|(p, _)| p.as_str()),
        work.is_some().then_some(path),
        bb,
        wb,
        Some(&head_bytes),
    )))
}

#[cfg(test)]
mod tests;
