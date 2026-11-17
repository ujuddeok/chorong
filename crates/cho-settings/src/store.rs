//! Loading, saving and watching `settings.json` and `repos.json`.
//!
//! The store is a plain single-owner value: no threads, no GUI. The app owns one, calls
//! [`SettingsStore::reload_if_changed`] on a debounced timer (a `stat` of two files when
//! nothing changed), and reads [`SettingsStore::effective`].

use crate::defs::{Value, def};
use crate::jsonc::{self, EditError, Node, NodeKind};
use crate::resolve::{Effective, Layer, resolve};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const SETTINGS_FILE: &str = "settings.json";
pub const REPOS_FILE: &str = "repos.json";

/// Which of Chorong's files a diagnostic is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Settings,
    Repos,
}

impl FileKind {
    pub fn file_name(&self) -> &'static str {
        match self {
            FileKind::Settings => SETTINGS_FILE,
            FileKind::Repos => REPOS_FILE,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// Syntax error. The last good settings stay in force.
    Malformed,
    /// The file exists but couldn't be read. The last good settings stay in force.
    Unreadable,
    /// A key Chorong doesn't know. Kept in the file, ignored.
    UnknownKey,
    /// A known key with a value of the wrong type or out of range. The value is ignored.
    InvalidValue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: FileKind,
    pub kind: DiagnosticKind,
    pub key: Option<String>,
    /// For `repos.json`: the repository the entry is for.
    pub repo: Option<String>,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.file.file_name(), self.message)
    }
}

/// What state a file is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileStatus {
    /// Not there; defaults apply.
    Missing,
    Ok,
    /// Syntax error; last good settings in force.
    Malformed(String),
    Unreadable(String),
}

impl FileStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            FileStatus::Missing => "missing",
            FileStatus::Ok => "ok",
            FileStatus::Malformed(_) => "malformed",
            FileStatus::Unreadable(_) => "unreadable",
        }
    }
}

/// Where a write goes.
#[derive(Clone, Copy, Debug)]
pub enum Scope<'a> {
    /// `settings.json`.
    Global,
    /// This repository's entry in `repos.json`. The path must exist.
    Repo(&'a Path),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveError {
    UnknownKey(String),
    InvalidValue(String),
    NoConfigDir,
    BadRepo(String),
    /// The file on disk has a syntax error; Chorong won't overwrite it.
    Malformed(String),
    Io(String),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::UnknownKey(k) => match crate::defs::retired(k) {
                Some(why) => write!(f, "`{k}` was retired: {why}"),
                None => write!(f, "unknown setting `{k}`"),
            },
            SaveError::InvalidValue(m) => f.write_str(m),
            SaveError::NoConfigDir => {
                f.write_str("no config directory (set CHORONG_CONFIG_DIR, XDG_CONFIG_HOME or HOME)")
            }
            SaveError::BadRepo(m) => f.write_str(m),
            SaveError::Malformed(m) => {
                write!(
                    f,
                    "not saved: the file has a syntax error and was left untouched ({m})"
                )
            }
            SaveError::Io(m) => write!(f, "could not write: {m}"),
        }
    }
}

impl std::error::Error for SaveError {}

/// What a mutation or reload changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    /// The store's version after the change.
    pub version: u64,
    /// Settings whose effective value changed, in display order.
    pub keys: Vec<&'static str>,
    /// The diagnostics list is different from before.
    pub diagnostics_changed: bool,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && !self.diagnostics_changed
    }

    pub fn contains(&self, key: &str) -> bool {
        self.keys.contains(&key)
    }
}

type Fingerprint = Option<(SystemTime, u64)>;

fn fingerprint(path: &Path) -> Fingerprint {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

struct FileState {
    kind: FileKind,
    path: Option<PathBuf>,
    seen: Fingerprint,
    status: FileStatus,
    diagnostics: Vec<Diagnostic>,
}

impl FileState {
    fn new(dir: Option<&Path>, kind: FileKind) -> Self {
        Self {
            kind,
            path: dir.map(|d| d.join(kind.file_name())),
            seen: None,
            status: FileStatus::Missing,
            diagnostics: Vec::new(),
        }
    }
}

/// Canonical form of a repository path, the key under which its overrides are stored.
/// (Non-UTF-8 paths are stored lossily.)
pub fn repo_key(path: &Path) -> Result<String, SaveError> {
    path.canonicalize()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| SaveError::BadRepo(format!("{}: {e}", path.display())))
}

pub struct SettingsStore {
    dir: Option<PathBuf>,
    global: Layer,
    global_file: FileState,
    repos: BTreeMap<String, Layer>,
    repos_file: FileState,
    active_repo: Option<String>,
    effective: Effective,
    version: u64,
}

impl SettingsStore {
    /// Open the store in `dir` and load both files. A missing directory or file is fine.
    pub fn open(dir: Option<PathBuf>) -> Self {
        let mut s = Self {
            global_file: FileState::new(dir.as_deref(), FileKind::Settings),
            repos_file: FileState::new(dir.as_deref(), FileKind::Repos),
            dir,
            global: Layer::default(),
            repos: BTreeMap::new(),
            active_repo: None,
            effective: resolve(&Layer::default(), None),
            version: 0,
        };
        s.load_global();
        s.load_repos();
        s.effective = s.compute();
        s
    }

    /// Open in this machine's config directory.
    pub fn open_default() -> Self {
        Self::open(crate::config_dir())
    }

    /// A store with no files: defaults only, and saves fail with [`SaveError::NoConfigDir`].
    pub fn in_memory() -> Self {
        Self::open(None)
    }

    pub fn config_dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    pub fn effective(&self) -> &Effective {
        &self.effective
    }

    /// Increases by one every time an effective value changes. Cheap to poll.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Everything wrong or worth knowing about the files, in file order.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.global_file
            .diagnostics
            .iter()
            .chain(&self.repos_file.diagnostics)
            .cloned()
            .collect()
    }

    pub fn unknown_keys(&self) -> Vec<Diagnostic> {
        self.diagnostics()
            .into_iter()
            .filter(|d| d.kind == DiagnosticKind::UnknownKey)
            .collect()
    }

    pub fn file_path(&self, which: FileKind) -> Option<&Path> {
        self.file(which).path.as_deref()
    }

    pub fn file_status(&self, which: FileKind) -> &FileStatus {
        &self.file(which).status
    }

    pub fn active_repo(&self) -> Option<&str> {
        self.active_repo.as_deref()
    }

    /// The value set in `settings.json` for `key`, if any.
    pub fn global_value(&self, key: &str) -> Option<Value> {
        self.global.get(key)
    }

    /// The value set for the active repository for `key`, if any.
    pub fn repo_value(&self, key: &str) -> Option<Value> {
        self.active_layer().and_then(|l| l.get(key))
    }

    /// Choose which repository's overrides apply (`None` for none).
    pub fn set_active_repo(&mut self, repo: Option<&Path>) -> Result<ChangeSet, SaveError> {
        let key = repo.map(repo_key).transpose()?;
        let before = self.snapshot();
        self.active_repo = key;
        Ok(self.finish(before))
    }

    /// Call on a debounced timer (a second or so) to pick up edits made outside Chorong.
    /// Costs two `stat`s when nothing changed and returns `None`. When a file changed, it is
    /// re-read; a file that no longer parses leaves the last good settings in force.
    pub fn reload_if_changed(&mut self) -> Option<ChangeSet> {
        let g = self.global_file.path.as_deref().map(fingerprint);
        let r = self.repos_file.path.as_deref().map(fingerprint);
        let g_changed = g.is_some_and(|f| f != self.global_file.seen);
        let r_changed = r.is_some_and(|f| f != self.repos_file.seen);
        if !g_changed && !r_changed {
            return None;
        }
        let before = self.snapshot();
        if g_changed {
            self.load_global();
        }
        if r_changed {
            self.load_repos();
        }
        Some(self.finish(before))
    }

    /// Write `value` for `key`, changing only that key in the file.
    pub fn set(
        &mut self,
        scope: Scope<'_>,
        key: &str,
        value: Value,
    ) -> Result<ChangeSet, SaveError> {
        let d = def(key).ok_or_else(|| SaveError::UnknownKey(key.to_string()))?;
        d.kind
            .check(&value)
            .map_err(|m| SaveError::InvalidValue(format!("{key}: {m}")))?;
        let literal = value.literal();
        self.edit(scope, key, |text, path| jsonc::set(text, path, &literal))
    }

    /// Remove `key` from the file, so the next layer down applies.
    pub fn unset(&mut self, scope: Scope<'_>, key: &str) -> Result<ChangeSet, SaveError> {
        def(key).ok_or_else(|| SaveError::UnknownKey(key.to_string()))?;
        self.edit(scope, key, jsonc::remove)
    }

    #[allow(clippy::disallowed_methods)] // Chorong's own settings file
    fn edit(
        &mut self,
        scope: Scope<'_>,
        key: &str,
        apply: impl Fn(&str, &[&str]) -> Result<String, EditError>,
    ) -> Result<ChangeSet, SaveError> {
        let (which, repo) = match scope {
            Scope::Global => (FileKind::Settings, None),
            Scope::Repo(p) => (FileKind::Repos, Some(repo_key(p)?)),
        };
        let path = self
            .file(which)
            .path
            .clone()
            .ok_or(SaveError::NoConfigDir)?;
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(SaveError::Io(format!("{}: {e}", path.display()))),
        };
        let edited = match &repo {
            None => apply(&text, &[key]),
            Some(r) => apply(&text, &["repos", r.as_str(), key]),
        }
        .map_err(|e| match e {
            EditError::Parse(p) => SaveError::Malformed(p.to_string()),
            EditError::Shape(m) => SaveError::Malformed(m),
        })?;
        let before = self.snapshot();
        if edited != text {
            write_atomic(&path, &edited)?;
        }
        match which {
            FileKind::Settings => self.load_global(),
            FileKind::Repos => self.load_repos(),
        }
        Ok(self.finish(before))
    }

    // ---- internals ----

    fn file(&self, which: FileKind) -> &FileState {
        match which {
            FileKind::Settings => &self.global_file,
            FileKind::Repos => &self.repos_file,
        }
    }

    fn active_layer(&self) -> Option<&Layer> {
        self.active_repo.as_ref().and_then(|r| self.repos.get(r))
    }

    fn compute(&self) -> Effective {
        resolve(&self.global, self.active_layer())
    }

    fn snapshot(&self) -> (Effective, Vec<Diagnostic>) {
        (self.effective.clone(), self.diagnostics())
    }

    fn finish(&mut self, (before, diags): (Effective, Vec<Diagnostic>)) -> ChangeSet {
        let now = self.compute();
        let keys = before.diff(&now);
        self.effective = now;
        if !keys.is_empty() {
            self.version += 1;
        }
        ChangeSet {
            version: self.version,
            keys,
            diagnostics_changed: diags != self.diagnostics(),
        }
    }

    fn load_global(&mut self) {
        let outcome = read_node(&mut self.global_file);
        if let Some(root) = outcome {
            let mut diags = Vec::new();
            self.global = layer_from(&root, FileKind::Settings, None, &mut diags);
            self.global_file.diagnostics = diags;
        }
    }

    fn load_repos(&mut self) {
        let outcome = read_node(&mut self.repos_file);
        let Some(root) = outcome else { return };
        let mut diags = Vec::new();
        let mut repos = BTreeMap::new();
        for m in root.members().unwrap_or_default() {
            if m.key != "repos" {
                diags.push(Diagnostic {
                    file: FileKind::Repos,
                    kind: DiagnosticKind::UnknownKey,
                    key: Some(m.key.clone()),
                    repo: None,
                    message: format!("unknown top-level key `{}` (kept)", m.key),
                });
                continue;
            }
            let Some(entries) = m.value.members() else {
                diags.push(Diagnostic {
                    file: FileKind::Repos,
                    kind: DiagnosticKind::InvalidValue,
                    key: Some("repos".into()),
                    repo: None,
                    message: "`repos` should be an object keyed by repository path".into(),
                });
                continue;
            };
            for e in entries {
                if e.value.members().is_none() {
                    diags.push(Diagnostic {
                        file: FileKind::Repos,
                        kind: DiagnosticKind::InvalidValue,
                        key: None,
                        repo: Some(e.key.clone()),
                        message: format!("the entry for `{}` should be an object", e.key),
                    });
                    continue;
                }
                let layer = layer_from(&e.value, FileKind::Repos, Some(&e.key), &mut diags);
                repos.insert(e.key.clone(), layer);
            }
        }
        self.repos = repos;
        self.repos_file.diagnostics = diags;
    }
}

/// Read and parse a file into its root object, updating its status. `None` means "keep the
/// last good layer" (malformed or unreadable); a missing or empty file gives an empty object.
#[allow(clippy::disallowed_methods)] // Chorong's own settings file
fn read_node(state: &mut FileState) -> Option<Node> {
    let which = state.kind;
    let Some(path) = state.path.clone() else {
        state.status = FileStatus::Missing;
        return Some(empty_object());
    };
    state.seen = fingerprint(&path);
    // Structural problems from an earlier read are replaced; per-key ones are kept on failure.
    let keep_key_diags = |state: &mut FileState| {
        state.diagnostics.retain(|d| {
            matches!(
                d.kind,
                DiagnosticKind::UnknownKey | DiagnosticKind::InvalidValue
            )
        });
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            state.status = FileStatus::Missing;
            state.diagnostics.clear();
            return Some(empty_object());
        }
        Err(e) => {
            let msg = e.to_string();
            keep_key_diags(state);
            state.diagnostics.push(Diagnostic {
                file: which,
                kind: DiagnosticKind::Unreadable,
                key: None,
                repo: None,
                message: format!("could not read the file: {msg}; keeping the last good settings"),
            });
            state.status = FileStatus::Unreadable(msg);
            return None;
        }
    };
    let malformed = |state: &mut FileState, msg: String| {
        keep_key_diags(state);
        state.diagnostics.push(Diagnostic {
            file: which,
            kind: DiagnosticKind::Malformed,
            key: None,
            repo: None,
            message: format!("{msg}; keeping the last good settings"),
        });
        state.status = FileStatus::Malformed(msg);
    };
    match jsonc::parse(&text) {
        Err(e) => {
            malformed(state, e.to_string());
            None
        }
        Ok(None) => {
            state.status = FileStatus::Ok;
            Some(empty_object())
        }
        Ok(Some(root)) if root.members().is_some() => {
            state.status = FileStatus::Ok;
            Some(root)
        }
        Ok(Some(_)) => {
            malformed(state, "the top-level value must be an object".into());
            None
        }
    }
}

fn empty_object() -> Node {
    Node {
        span: 0..0,
        kind: NodeKind::Object(Vec::new()),
    }
}

/// Build a layer from an object's members; unknown keys and bad values become diagnostics.
fn layer_from(
    obj: &Node,
    file: FileKind,
    repo: Option<&str>,
    diags: &mut Vec<Diagnostic>,
) -> Layer {
    let mut layer = Layer::default();
    let mut reported: Vec<&str> = Vec::new();
    for m in obj.members().unwrap_or_default() {
        let diag = |kind, message: String| Diagnostic {
            file,
            kind,
            key: Some(m.key.clone()),
            repo: repo.map(str::to_string),
            message,
        };
        let Some(d) = def(&m.key) else {
            if let Some(why) = crate::defs::retired(&m.key) {
                if !reported.contains(&m.key.as_str()) {
                    reported.push(&m.key);
                    diags.push(diag(
                        DiagnosticKind::UnknownKey,
                        format!("`{}` was retired (kept, ignored): {why}", m.key),
                    ));
                }
                continue;
            }
            if !reported.contains(&m.key.as_str()) {
                reported.push(&m.key);
                let hint = if m.value.members().is_some() {
                    "; nested objects aren't supported, write dotted keys such as \"context.follow_hover\""
                } else {
                    ""
                };
                diags.push(diag(
                    DiagnosticKind::UnknownKey,
                    format!("unknown setting `{}` (kept, ignored){hint}", m.key),
                ));
            }
            continue;
        };
        let value = match &m.value.kind {
            NodeKind::Bool(b) => Some(Value::Bool(*b)),
            NodeKind::Number(raw) if raw.bytes().all(|b| b.is_ascii_digit()) => {
                raw.parse::<u64>().ok().map(Value::UInt)
            }
            NodeKind::Str(s) => Some(Value::text(s.clone())),
            _ => None,
        };
        match value.map(|v| d.kind.check(&v).map(|()| v)) {
            Some(Ok(v)) => layer.put(&m.key, v),
            Some(Err(why)) => diags.push(diag(
                DiagnosticKind::InvalidValue,
                format!("{}: {why}; using the next layer down", m.key),
            )),
            None => diags.push(diag(
                DiagnosticKind::InvalidValue,
                format!(
                    "{}: expected {}; using the next layer down",
                    m.key,
                    match d.kind {
                        crate::Kind::Bool => "true or false".to_string(),
                        crate::Kind::UInt { min, max } =>
                            format!("a whole number from {min} to {max}"),
                        crate::Kind::Font { .. } => "a font family name in quotes".to_string(),
                        crate::Kind::Path => "an absolute path in quotes, or \"\"".to_string(),
                        crate::Kind::Version =>
                            "a version in quotes (0.6.0, ^0.6.0, ~0.6.0 or latest)".to_string(),
                        crate::Kind::Choice { options } =>
                            format!("one of {} in quotes", options.join(", ")),
                    }
                ),
            )),
        }
    }
    layer
}

fn write_atomic(path: &Path, text: &str) -> Result<(), SaveError> {
    let io = |e: std::io::Error| SaveError::Io(format!("{}: {e}", path.display()));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let tmp = path.with_extension(format!("json.tmp{}", std::process::id()));
    std::fs::write(&tmp, text).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io(e)
    })
}
