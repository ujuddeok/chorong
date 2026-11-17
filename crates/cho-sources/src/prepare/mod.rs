//! Prepare: the commands a remote worktree needs before it can be indexed (a package install,
//! a build for generated code), and the approval that lets them, and the indexers, run.
//!
//! - A [`Recipe`] is an ordered list of [`Step`]s, each an argv run with no shell.
//! - Where the recipe comes from ([`effective`]), highest first: the project's own recipe,
//!   the repository's `.chorong/prepare.json` at the commit being prepared, a suggestion from a
//!   lockfile, or none.
//! - One [`Approval`] per project covers prepare and indexing. It records a hash of the
//!   recipe and of the trigger files ([`triggers`]: lockfiles, `package.json`, `build.rs`,
//!   proc-macro `Cargo.toml`, `.chorong/prepare.json`) and is asked again when either differs.
//! - Running is in [`run`]: only in Chorong's own worktrees, with tokens removed from the
//!   environment, a timeout per step and a log in the cache.

pub mod ops;
pub mod run;

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

/// The repository's own recipe, read from the commit being prepared.
pub const PREPARE_FILE: &str = ".chorong/prepare.json";
/// Seconds a step may run when the recipe doesn't say.
pub const DEFAULT_STEP_TIMEOUT: u64 = 1800;

/// One command, run as argv (no shell) in `cwd` (relative to the worktree).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub argv: Vec<String>,
    #[serde(default = "dot")]
    pub cwd: String,
    #[serde(default = "default_timeout")]
    pub timeout_s: u64,
}

fn dot() -> String {
    ".".into()
}

fn default_timeout() -> u64 {
    DEFAULT_STEP_TIMEOUT
}

impl Step {
    pub fn new(argv: &[&str]) -> Self {
        Self {
            argv: argv.iter().map(|s| s.to_string()).collect(),
            cwd: dot(),
            timeout_s: DEFAULT_STEP_TIMEOUT,
        }
    }

    /// The command as a person would read it, in shell syntax: arguments with spaces or
    /// quotes in `'…'`, and arguments with control or bidirectional-override characters in
    /// `$'…'` with those characters escaped, so the approval prompt can't be made to show
    /// a different command than the one that runs.
    pub fn display(&self) -> String {
        let args = self
            .argv
            .iter()
            .map(|a| {
                if a.contains(hides_text) {
                    ansi_c_quote(a)
                } else if a.is_empty()
                    || a.contains(|c: char| c.is_whitespace() || "'\"\\$`".contains(c))
                {
                    format!("'{}'", a.replace('\'', r"'\''"))
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        if self.cwd == "." {
            args
        } else {
            format!("(in {}) {args}", self.cwd)
        }
    }
}

/// Characters that change how the rest of a line displays: control characters (escape
/// sequences, newlines, delete) and bidirectional overrides.
fn hides_text(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// `$'…'` quoting (bash, zsh, POSIX 2024) with every [`hides_text`] character escaped.
fn ansi_c_quote(a: &str) -> String {
    let mut out = String::from("$'");
    for c in a.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            c if hides_text(c) && (c as u32) < 0x100 => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c if hides_text(c) => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// An ordered list of steps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recipe {
    pub steps: Vec<Step>,
}

impl Recipe {
    /// Parses `.chorong/prepare.json` or a `--file` recipe: `{"steps": [{"argv": [...]}, ...]}`.
    pub fn parse(text: &str) -> Result<Self> {
        let r: Recipe = serde_json::from_str(text)
            .map_err(|e| Error::msg(format!("not a valid recipe: {e}")))?;
        r.validate()?;
        Ok(r)
    }

    /// Each step has a program, a relative `cwd` that stays in the worktree, and a timeout.
    pub fn validate(&self) -> Result<()> {
        for (i, s) in self.steps.iter().enumerate() {
            let n = i + 1;
            if s.argv.first().is_none_or(|p| p.trim().is_empty()) {
                return Err(Error::msg(format!("step {n} has no program")));
            }
            if s.argv.iter().any(|a| a.contains('\0')) {
                return Err(Error::msg(format!("step {n} has a NUL character")));
            }
            let cwd = Path::new(&s.cwd);
            if cwd.is_absolute()
                || s.cwd.contains('\\')
                || s.cwd.contains(hides_text)
                || cwd
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(Error::msg(format!(
                    "step {n}: cwd `{}` must be a folder inside the repository",
                    s.cwd
                )));
            }
            if s.timeout_s == 0 {
                return Err(Error::msg(format!(
                    "step {n}: timeout_s must be at least 1"
                )));
            }
        }
        Ok(())
    }

    /// SHA-256 of the recipe's canonical JSON: what an approval is stored against.
    pub fn hash(&self) -> String {
        sha256_hex(serde_json::to_string(self).expect("json").as_bytes())
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Where the effective recipe came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecipeSource {
    /// The project's own recipe in `projects.json`.
    Project,
    /// The repository's `.chorong/prepare.json` at the commit.
    Repository,
    /// Suggested from a lockfile; approving it saves it as the project's recipe.
    Suggested,
    /// Nothing to run; indexing runs as it is.
    None,
}

impl RecipeSource {
    /// Where the recipe comes from, in words, for the CLI and the viewer alike. `commit` is
    /// the commit the repository's file was read at.
    pub fn describe(self, commit: &str) -> String {
        match self {
            Self::Project => "the project's own recipe".to_string(),
            Self::Repository => format!(
                "the repository's {PREPARE_FILE} at {}",
                cho_git::short(commit)
            ),
            Self::Suggested => {
                "suggested from the lockfile (approving keeps it as the project's recipe)".into()
            }
            Self::None => "none: nothing to run before indexing".into(),
        }
    }
}

/// The recipe that applies at a commit, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Effective {
    pub recipe: Recipe,
    pub source: RecipeSource,
    /// Why, when it is not obvious (several lockfiles, an invalid `.chorong/prepare.json`).
    pub notes: Vec<String>,
}

/// Lockfiles at the repository root and the command each suggests, in order of preference.
const LOCKFILES: [(&str, &[&str]); 3] = [
    ("pnpm-lock.yaml", &["pnpm", "install", "--frozen-lockfile"]),
    ("yarn.lock", &["yarn", "install", "--immutable"]),
    ("package-lock.json", &["npm", "ci"]),
];

/// uv's lockfile, and the command it suggests: exactly the lockfile, into the project's
/// `.venv` (it may download the Python that `.python-version` names).
const UV_LOCK: (&str, &[&str]) = ("uv.lock", &["uv", "sync", "--frozen"]);

/// The suggested recipe for the files at the repository root, if a lockfile names one: the
/// JavaScript install (several lockfiles: the first in the order pnpm, yarn, npm, with a
/// note), then `uv sync --frozen` when there is a `uv.lock`.
pub fn suggestion(root_files: &[&str]) -> Option<(Recipe, Option<String>)> {
    let found: Vec<&(&str, &[&str])> = LOCKFILES
        .iter()
        .filter(|(f, _)| root_files.contains(f))
        .collect();
    let note = (found.len() > 1).then(|| {
        format!(
            "Several lockfiles ({}); suggesting the one for {}.",
            found.iter().map(|(f, _)| *f).collect::<Vec<_>>().join(", "),
            found[0].0
        )
    });
    let mut steps: Vec<Step> = found
        .first()
        .map(|(_, argv)| Step::new(argv))
        .into_iter()
        .collect();
    if root_files.contains(&UV_LOCK.0) {
        steps.push(Step::new(UV_LOCK.1));
    }
    (!steps.is_empty()).then_some((Recipe { steps }, note))
}

/// The recipe that applies at `commit` in the repository at `repo` (the cache's bare clone).
pub fn effective(
    git: &crate::Git,
    repo: &Path,
    commit: &str,
    project_recipe: Option<&Recipe>,
) -> Result<Effective> {
    let mut notes = vec![];
    if let Some(r) = project_recipe {
        return Ok(Effective {
            recipe: r.clone(),
            source: RecipeSource::Project,
            notes,
        });
    }
    match crate::reads::read_file_at(git, repo, commit, PREPARE_FILE) {
        Ok(bytes) => match Recipe::parse(&String::from_utf8_lossy(&bytes)) {
            Ok(r) => {
                return Ok(Effective {
                    recipe: r,
                    source: RecipeSource::Repository,
                    notes,
                });
            }
            Err(e) => notes.push(format!("{PREPARE_FILE} is ignored: {e}")),
        },
        Err(e) if e.kind == crate::ErrorKind::NotFound => {}
        // Not a file (a folder of that name): ignored like an invalid recipe.
        Err(e) if e.kind == crate::ErrorKind::Unsupported => {
            notes.push(format!("{PREPARE_FILE} is ignored: {e}"));
        }
        Err(e) => return Err(e),
    }
    let root = root_files(repo, commit)?;
    let names: Vec<&str> = root.iter().map(String::as_str).collect();
    if let Some((recipe, note)) = suggestion(&names) {
        notes.extend(note);
        return Ok(Effective {
            recipe,
            source: RecipeSource::Suggested,
            notes,
        });
    }
    Ok(Effective {
        recipe: Recipe::default(),
        source: RecipeSource::None,
        notes,
    })
}

fn open_repo(repo: &Path) -> Result<gix::Repository> {
    gix::open(repo).map_err(|e| Error::msg(format!("cannot open {}: {e}", repo.display())))
}

fn tree_at<'r>(r: &'r gix::Repository, commit: &str) -> Result<gix::Tree<'r>> {
    let id = gix::ObjectId::from_hex(commit.as_bytes())
        .map_err(|e| Error::msg(format!("`{commit}`: {e}")))?;
    r.find_commit(id)
        .and_then(|c| c.tree())
        .map_err(|e| Error::msg(format!("commit {commit}: {e}")))
}

/// Names of the files at the root of `commit`.
fn root_files(repo: &Path, commit: &str) -> Result<Vec<String>> {
    let r = open_repo(repo)?;
    let tree = tree_at(&r, commit)?;
    let mut out = vec![];
    for e in tree.iter() {
        let e = e.map_err(|e| Error::msg(e.to_string()))?;
        if e.mode().is_blob() {
            out.push(e.filename().to_string());
        }
    }
    Ok(out)
}

/// Trigger files: path → blob id. A change to any of them can change what prepare or an
/// indexer runs, so approval is asked again.
pub type Triggers = BTreeMap<String, String>;

/// File names that are always triggers wherever they are.
const TRIGGER_NAMES: [&str; 10] = [
    "package.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "build.rs",
    "Cargo.lock",
    // Python with uv: the lockfile, the project (its dependencies and build backend) and the
    // Python version `uv sync` may download.
    ".python-version",
    "pyproject.toml",
    "uv.lock",
];

/// The trigger files at `commit`: every file named in [`TRIGGER_NAMES`], `.chorong/prepare.json`,
/// and the `Cargo.toml` of each proc-macro crate. Read from the object store (trees are in the
/// blobless clone; a `Cargo.toml`'s contents are fetched if needed).
pub fn triggers(git: &crate::Git, repo: &Path, commit: &str) -> Result<Triggers> {
    let r = open_repo(repo)?;
    let tree = tree_at(&r, commit)?;
    let mut rec = gix::traverse::tree::Recorder::default();
    tree.traverse()
        .breadthfirst(&mut rec)
        .map_err(|e| Error::msg(format!("reading the tree of {commit}: {e}")))?;
    let mut out = Triggers::new();
    for e in rec.records {
        if !e.mode.is_blob() {
            continue;
        }
        let path = e.filepath.to_string();
        let name = path.rsplit('/').next().unwrap_or(&path);
        let wanted = path == PREPARE_FILE
            || TRIGGER_NAMES.contains(&name)
            || (name == "Cargo.toml" && is_proc_macro(git, repo, commit, &path)?);
        if wanted {
            out.insert(path, e.oid.to_string());
        }
    }
    Ok(out)
}

/// A `Cargo.toml` that declares `proc-macro = true` (or `proc_macro`) in any table.
fn is_proc_macro(git: &crate::Git, repo: &Path, commit: &str, path: &str) -> Result<bool> {
    let bytes = crate::reads::read_file_at(git, repo, commit, path)?;
    Ok(String::from_utf8_lossy(&bytes).lines().any(|l| {
        let l: String = l.chars().filter(|c| !c.is_whitespace()).collect();
        l.starts_with("proc-macro=true") || l.starts_with("proc_macro=true")
    }))
}

/// SHA-256 over the sorted `path\0blob\n` lines.
pub fn triggers_hash(t: &Triggers) -> String {
    let mut text = String::new();
    for (p, b) in t {
        text.push_str(p);
        text.push('\0');
        text.push_str(b);
        text.push('\n');
    }
    sha256_hex(text.as_bytes())
}

/// What a project's owner approved: one approval covers prepare and indexing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub recipe_hash: String,
    /// The recipe as approved, so a change can be shown.
    pub recipe: Recipe,
    pub source: RecipeSource,
    pub triggers: Triggers,
    pub triggers_hash: String,
    /// The commit it was approved at.
    pub commit: String,
    /// Seconds since the Unix epoch.
    pub approved_at: u64,
}

/// One trigger file that differs from the approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TriggerChange {
    pub path: String,
    pub change: Change,
}

/// How a trigger file differs from the approval. In JSON and words: `added`, `removed`,
/// `changed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Change {
    Added,
    Removed,
    Changed,
}

impl Change {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::Changed => "changed",
        }
    }
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether the project's approval covers a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum ApprovalState {
    Approved,
    NotApproved,
    /// Approved before, but the recipe or trigger files differ: ask again.
    Changed {
        recipe: bool,
        triggers: Vec<TriggerChange>,
    },
}

impl ApprovalState {
    pub fn is_approved(&self) -> bool {
        matches!(self, Self::Approved)
    }
}

/// Compares an approval with the recipe and trigger files at a commit. Deterministic: lists
/// exactly which trigger files changed, by path.
pub fn check(approval: Option<&Approval>, eff: &Effective, t: &Triggers) -> ApprovalState {
    let Some(a) = approval else {
        return ApprovalState::NotApproved;
    };
    let recipe = a.recipe_hash != eff.recipe.hash();
    let mut changes = vec![];
    for (p, b) in t {
        match a.triggers.get(p) {
            None => changes.push(TriggerChange {
                path: p.clone(),
                change: Change::Added,
            }),
            Some(old) if old != b => changes.push(TriggerChange {
                path: p.clone(),
                change: Change::Changed,
            }),
            _ => {}
        }
    }
    for p in a.triggers.keys().filter(|p| !t.contains_key(*p)) {
        changes.push(TriggerChange {
            path: p.clone(),
            change: Change::Removed,
        });
    }
    changes.sort_by(|x, y| x.path.cmp(&y.path));
    if !recipe && changes.is_empty() {
        ApprovalState::Approved
    } else {
        ApprovalState::Changed {
            recipe,
            triggers: changes,
        }
    }
}

/// A new approval for `eff` and `t` at `commit`.
pub fn approve(eff: &Effective, t: &Triggers, commit: &str) -> Approval {
    Approval {
        recipe_hash: eff.recipe.hash(),
        recipe: eff.recipe.clone(),
        source: eff.source,
        triggers: t.clone(),
        triggers_hash: triggers_hash(t),
        commit: commit.to_string(),
        approved_at: crate::now_unix(),
    }
}

#[cfg(test)]
mod tests;
