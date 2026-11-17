//! JavaScript/TypeScript workspace packages (pnpm `pnpm-workspace.yaml`, npm/yarn
//! `package.json` `workspaces`): each package's name, version, directory, entry points
//! and build output directories. Used to join a package's built declarations (`dist`)
//! to its source; see [`crate::dist_link`].
//!
//! Package directories come from the workspace globs matched against the directories
//! that hold indexed documents, so nothing walks the file system and only packages the
//! index has source for are found. Every manifest is read with `cho_core::fs`.

use crate::relpath::{ancestors, join, normalize, parent, trim_root, within};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

/// One workspace package. Paths are relative to the package directory, `/`-separated,
/// normalised (no `./`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspacePackage {
    pub name: String,
    /// `None` when `package.json` has no version.
    pub version: Option<String>,
    /// Repository-relative, `/`-separated; empty for the repository root.
    pub dir: String,
    /// Entry points: a built declaration file and, when the manifest names one, the source
    /// file for the same entry (`source` field or a `source` export condition).
    pub entries: Vec<Entry>,
    /// tsconfig `outDir`, `declarationDir` and `rootDir`, package-relative.
    pub tsconfig: TsDirs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `types`/`typings`, or an export's `types` target (`dist/index.d.ts`).
    pub types: String,
    /// The source for the same entry, if the manifest names one.
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TsDirs {
    pub out_dir: Option<String>,
    pub declaration_dir: Option<String>,
    pub root_dir: Option<String>,
}

impl WorkspacePackage {
    /// Directories built declarations are written to: tsconfig `declarationDir` and
    /// `outDir`, the first directory of each entry's declaration file, and `dist`.
    pub fn output_dirs(&self) -> Vec<String> {
        let mut dirs: Vec<String> = vec![];
        let mut add = |d: &str| {
            let d = d.trim_matches('/');
            if !d.is_empty() && d != "." && !dirs.iter().any(|x| x == d) {
                dirs.push(d.to_string());
            }
        };
        if let Some(d) = &self.tsconfig.declaration_dir {
            add(d);
        }
        if let Some(d) = &self.tsconfig.out_dir {
            add(d);
        }
        for e in &self.entries {
            if let Some((first, _)) = e.types.split_once('/') {
                add(first);
            }
        }
        add("dist");
        dirs
    }

    /// Does the version in a SCIP symbol name this package's version? scip-typescript
    /// writes `HEAD` when `package.json` has none.
    pub fn version_matches(&self, version: &str) -> bool {
        match &self.version {
            Some(v) => v == version,
            None => version == "HEAD",
        }
    }
}

/// The workspace packages of the repository at `root` whose directories hold one of
/// `doc_dirs` (repository-relative directories of indexed documents). Workspace files
/// are looked for at the repository root and at each of `project_roots`.
pub fn detect(
    root: &Path,
    project_roots: &[String],
    doc_dirs: &BTreeSet<String>,
) -> Vec<WorkspacePackage> {
    let mut bases: Vec<String> = vec![String::new()];
    for p in project_roots {
        let p = normalize(trim_root(p)).unwrap_or_default();
        if !bases.contains(&p) {
            bases.push(p);
        }
    }
    // Every ancestor of an indexed document is a candidate package directory.
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    for d in doc_dirs {
        for a in ancestors(d) {
            if !dirs.insert(a.to_string()) {
                break;
            }
        }
    }
    let mut found: Vec<WorkspacePackage> = vec![];
    for base in &bases {
        let globs = workspace_globs(root, base);
        if globs.is_empty() {
            continue;
        }
        let (exclude, include): (Vec<&String>, Vec<&String>) =
            globs.iter().partition(|g| g.starts_with('!'));
        for dir in &dirs {
            let Some(rel) = within(dir, base) else {
                continue;
            };
            if rel.is_empty()
                || rel
                    .split('/')
                    .any(|s| s == "node_modules" || s.starts_with('.'))
            {
                continue;
            }
            let pat = |g: &str| glob_match(g.trim_start_matches("./").trim_end_matches('/'), rel);
            if !include.iter().any(|g| pat(g)) || exclude.iter().any(|g| pat(&g[1..])) {
                continue;
            }
            if found.iter().any(|p| &p.dir == dir) {
                continue;
            }
            if let Some(pkg) = read_package(root, dir) {
                found.push(pkg);
            }
        }
    }
    found
}

fn read_text(root: &Path, rel: &str) -> Option<String> {
    cho_core::fs::read_source_text_in(root, Path::new(rel)).ok()
}

/// The workspace globs declared at `base`: `pnpm-workspace.yaml` `packages`, else
/// `package.json` `workspaces` (an array, or an object with `packages`).
fn workspace_globs(root: &Path, base: &str) -> Vec<String> {
    if let Some(text) = read_text(root, &join(base, "pnpm-workspace.yaml")) {
        let globs = pnpm_packages(&text);
        if !globs.is_empty() {
            return globs;
        }
    }
    let Some(v) = read_text(root, &join(base, "package.json"))
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return vec![];
    };
    let list = match v.get("workspaces") {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Object(o)) => o
            .get("packages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => vec![],
    };
    list.iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// The `packages:` list of a `pnpm-workspace.yaml`: a block list (`- 'packages/*'`) or a
/// flow list (`['a', 'b']`). Only this key is read; the rest of the YAML is ignored.
pub fn pnpm_packages(text: &str) -> Vec<String> {
    let unquote = |s: &str| {
        let s = s.split(" #").next().unwrap_or(s).trim();
        s.trim_matches(|c| c == '\'' || c == '"').to_string()
    };
    let mut out = vec![];
    let mut in_list = false;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim_start().starts_with('#') || trimmed.trim().is_empty() {
            continue;
        }
        let indented = line.starts_with([' ', '\t', '-']);
        if !indented {
            in_list = false;
            if let Some(rest) = trimmed.strip_prefix("packages:") {
                let rest = rest.trim();
                if let Some(flow) = rest.strip_prefix('[') {
                    let flow = flow.split(']').next().unwrap_or("");
                    out.extend(flow.split(',').map(unquote).filter(|s| !s.is_empty()));
                } else {
                    in_list = true;
                }
            }
            continue;
        }
        if in_list && let Some(item) = trimmed.trim_start().strip_prefix('-') {
            let item = unquote(item);
            if !item.is_empty() {
                out.push(item);
            }
        }
    }
    out
}

/// Deepest folder below the root a pnpm workspace package is looked for in.
const MAX_PACKAGE_DEPTH: usize = 8;
/// Most folders read while listing a workspace's packages, so a huge tree stays cheap.
const MAX_PACKAGE_DIRS: usize = 20_000;

/// The TypeScript projects of the pnpm workspace at `root`, as `pnpm ls -r` lists them for
/// scip-typescript's `--pnpm-workspaces`: `root` first, then every folder with a
/// `package.json` that a `packages:` glob of `pnpm-workspace.yaml` matches (and no `!` glob
/// excludes), sorted by path; absolute. Empty when there is no `pnpm-workspace.yaml`. Chorong
/// lists them itself so that no `pnpm` runs in the repository. Hidden folders,
/// `node_modules` and links are not entered.
pub fn pnpm_projects(root: &Path) -> Vec<std::path::PathBuf> {
    if !root.join("pnpm-workspace.yaml").is_file() {
        return vec![];
    }
    let globs = read_text(root, "pnpm-workspace.yaml")
        .map(|t| pnpm_packages(&t))
        .unwrap_or_default();
    let clean = |g: &str| {
        let g = g.trim();
        let g = g.strip_prefix("./").unwrap_or(g);
        g.trim_end_matches('/').to_string()
    };
    let include: Vec<String> = globs
        .iter()
        .filter(|g| !g.starts_with('!'))
        .map(|g| clean(g))
        .filter(|g| !g.is_empty())
        .collect();
    let exclude: Vec<String> = globs
        .iter()
        .filter_map(|g| g.strip_prefix('!'))
        .map(clean)
        .collect();
    let deep = include.iter().any(|g| g.contains("**"));
    let depth = if deep {
        MAX_PACKAGE_DEPTH
    } else {
        include
            .iter()
            .map(|g| g.split('/').count())
            .max()
            .unwrap_or(0)
            .min(MAX_PACKAGE_DEPTH)
    };
    let mut found: Vec<String> = vec![];
    let mut queue: std::collections::VecDeque<(String, usize)> = [(String::new(), 0)].into();
    let mut read = 0;
    while let Some((rel, d)) = queue.pop_front() {
        if d >= depth || read >= MAX_PACKAGE_DIRS {
            continue;
        }
        read += 1;
        let Ok(rd) = std::fs::read_dir(root.join(&rel)) else {
            continue;
        };
        for e in rd.flatten() {
            // `file_type` does not follow links, so a linked folder is never entered.
            if !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            let child = join(&rel, &name);
            if include.iter().any(|g| glob_match(g, &child))
                && !exclude.iter().any(|g| glob_match(g, &child))
                && e.path().join("package.json").is_file()
            {
                found.push(child.clone());
            }
            queue.push_back((child, d + 1));
        }
    }
    found.sort();
    std::iter::once(root.to_path_buf())
        .chain(found.iter().map(|rel| root.join(rel)))
        .collect()
}

/// Matches a workspace glob against a `/`-separated directory: `*` matches within one
/// segment, `**` any number of segments.
pub fn glob_match(glob: &str, path: &str) -> bool {
    let g: Vec<&str> = glob.split('/').filter(|s| !s.is_empty()).collect();
    let p: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    wildcard(
        &g,
        &p,
        |g| *g == "**",
        |g, s| wildcard(g.as_bytes(), s.as_bytes(), |c| *c == b'*', |c, d| c == d),
    )
}

/// Does `pat` match all of `s`, where a star element matches any run of elements and every
/// other element matches one element it `eq`s? Greedy, backtracking only to the last star,
/// so time stays at most `pat.len() * s.len()` comparisons (globs come from the repository).
fn wildcard<P, S>(
    pat: &[P],
    s: &[S],
    is_star: impl Fn(&P) -> bool,
    eq: impl Fn(&P, &S) -> bool,
) -> bool {
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        if pi < pat.len() && is_star(&pat[pi]) {
            star = Some((pi, si));
            pi += 1;
        } else if pi < pat.len() && eq(&pat[pi], &s[si]) {
            pi += 1;
            si += 1;
        } else if let Some((sp, ss)) = star {
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, ss + 1));
        } else {
            return false;
        }
    }
    pat[pi..].iter().all(is_star)
}

/// Reads `dir/package.json` (and the package's tsconfig). `None` without a name.
pub fn read_package(root: &Path, dir: &str) -> Option<WorkspacePackage> {
    let v: Value = serde_json::from_str(&read_text(root, &join(dir, "package.json"))?).ok()?;
    let name = v.get("name")?.as_str()?.to_string();
    let version = v.get("version").and_then(Value::as_str).map(str::to_string);
    Some(WorkspacePackage {
        name,
        version,
        dir: dir.to_string(),
        entries: entries(&v),
        tsconfig: tsconfig_dirs(root, dir),
    })
}

/// Is `key` an export condition that names source (`source`, `@zod/source`)?
fn is_source_condition(key: &str) -> bool {
    key == "source" || key.ends_with("/source") || key.ends_with(":source")
}

/// TypeScript declaration-file suffixes, each with the source suffixes it is built from.
pub const DECLARATION_EXTS: [(&str, &[&str]); 3] = [
    (".d.ts", &[".ts", ".tsx"]),
    (".d.mts", &[".mts"]),
    (".d.cts", &[".cts"]),
];

pub fn is_declaration_file(path: &str) -> bool {
    DECLARATION_EXTS.iter().any(|(d, _)| path.ends_with(d))
}

/// The source files a declaration file is built from: `x.d.ts` to `x.ts` and `x.tsx`,
/// `.d.mts` to `.mts`, `.d.cts` to `.cts`. Empty for any other file.
pub fn source_names(decl: &str) -> Vec<String> {
    DECLARATION_EXTS
        .iter()
        .find_map(|(d, sources)| {
            let stem = decl.strip_suffix(d)?;
            Some(sources.iter().map(|s| format!("{stem}{s}")).collect())
        })
        .unwrap_or_default()
}

/// Entry points from a manifest: `types`/`typings` with the `source` field, and each
/// `exports` entry's declaration target with its source condition.
pub fn entries(manifest: &Value) -> Vec<Entry> {
    let mut out: Vec<Entry> = vec![];
    let field = |k: &str| manifest.get(k).and_then(Value::as_str).and_then(normalize);
    let source_field = field("source").filter(|s| !is_declaration_file(s) && !s.contains('*'));
    for k in ["types", "typings"] {
        if let Some(t) = field(k).filter(|t| is_declaration_file(t) && !t.contains('*')) {
            push_entry(&mut out, t, source_field.clone());
        }
    }
    // The `.` export's targets, or the whole of `exports` when it is a single target.
    match manifest.get("exports") {
        Some(Value::Object(o)) if o.keys().any(|k| k.starts_with('.')) => {
            for (sub, target) in o {
                let mut types = vec![];
                let mut source = None;
                collect_targets(target, false, &mut types, &mut source);
                let source =
                    source.or_else(|| (sub == ".").then(|| source_field.clone()).flatten());
                for t in types {
                    push_entry(&mut out, t, source.clone());
                }
            }
        }
        Some(target) => {
            let mut types = vec![];
            let mut source = None;
            collect_targets(target, false, &mut types, &mut source);
            for t in types {
                push_entry(&mut out, t, source.clone().or_else(|| source_field.clone()));
            }
        }
        None => {}
    }
    out
}

fn push_entry(out: &mut Vec<Entry>, types: String, source: Option<String>) {
    if let Some(e) = out.iter_mut().find(|e| e.types == types) {
        if e.source.is_none() {
            e.source = source;
        }
    } else {
        out.push(Entry { types, source });
    }
}

/// Walks one export's condition tree: declaration targets go to `types`, the target of a
/// source condition to `source`.
fn collect_targets(
    v: &Value,
    under_source: bool,
    types: &mut Vec<String>,
    source: &mut Option<String>,
) {
    match v {
        Value::String(s) => {
            let Some(p) = normalize(s) else { return };
            if under_source {
                if source.is_none() && !is_declaration_file(&p) && !p.contains('*') {
                    *source = Some(p);
                }
            } else if is_declaration_file(&p) && !p.contains('*') && !types.contains(&p) {
                types.push(p);
            }
        }
        Value::Object(o) => {
            for (k, t) in o {
                collect_targets(t, under_source || is_source_condition(k), types, source);
            }
        }
        Value::Array(a) => {
            for t in a {
                collect_targets(t, under_source, types, source);
            }
        }
        _ => {}
    }
}

/// tsconfig `outDir`, `declarationDir` and `rootDir` for the package at `dir`: from
/// `tsconfig.build.json`, else `tsconfig.json`, following relative `extends` (each
/// option relative to the file that sets it). Options outside the package are dropped.
fn tsconfig_dirs(root: &Path, dir: &str) -> TsDirs {
    for name in ["tsconfig.build.json", "tsconfig.json"] {
        let mut dirs = TsDirs::default();
        if !read_tsconfig(root, &join(dir, name), &mut dirs, 0) {
            continue;
        }
        let rel = |o: Option<String>| o.and_then(|p| within(&p, dir).map(str::to_string));
        let dirs = TsDirs {
            out_dir: rel(dirs.out_dir),
            declaration_dir: rel(dirs.declaration_dir),
            root_dir: rel(dirs.root_dir),
        };
        if dirs.out_dir.is_some() || dirs.declaration_dir.is_some() {
            return dirs;
        }
    }
    TsDirs::default()
}

/// Reads one tsconfig into `dirs` (repository-relative), options set nearer winning.
/// `false` when the file can't be read.
fn read_tsconfig(root: &Path, path: &str, dirs: &mut TsDirs, depth: usize) -> bool {
    if depth > 8 {
        return false;
    }
    let Some(v) = read_text(root, path).and_then(|t| cho_core::jsonc::parse_value(&t)) else {
        return false;
    };
    let here = parent(path);
    let opt = |k: &str| {
        v.get("compilerOptions")
            .and_then(|c| c.get(k))
            .and_then(Value::as_str)
            .and_then(|p| normalize(&join(here, p)))
    };
    if dirs.out_dir.is_none() {
        dirs.out_dir = opt("outDir");
    }
    if dirs.declaration_dir.is_none() {
        dirs.declaration_dir = opt("declarationDir");
    }
    if dirs.root_dir.is_none() {
        dirs.root_dir = opt("rootDir");
    }
    let extends: Vec<&str> = match v.get("extends") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    // Later entries of an `extends` array win, so read them first.
    for e in extends.into_iter().rev() {
        if !e.starts_with('.') {
            continue; // a package's config (`@tsconfig/node20`): outside the repository's own files
        }
        let mut target = join(here, e);
        if !target.ends_with(".json") {
            target.push_str(".json");
        }
        if let Some(t) = normalize(&target) {
            read_tsconfig(root, &t, dirs, depth + 1);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn glob_time_stays_linear() {
        // Found by the `workspace` fuzz target: runs of `*` backtracked exponentially, so a
        // repository's workspace glob could hang the dist-to-source join.
        let glob = format!("p/{}x/a", "*".repeat(90));
        let path = format!("p/{}y/a", "a".repeat(200));
        let started = std::time::Instant::now();
        assert!(!glob_match(&glob, &path));
        assert!(!glob_match(
            &format!("{}x", "**/".repeat(60)),
            &"a/".repeat(200)
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn globs() {
        assert!(glob_match("packages/*", "packages/a"));
        assert!(!glob_match("packages/*", "packages/a/b"));
        assert!(!glob_match("packages/*", "packages"));
        assert!(glob_match("packages/**", "packages/a/b"));
        assert!(glob_match("apps/web-*", "apps/web-x"));
        assert!(!glob_match("apps/web-*", "apps/api"));
        assert!(glob_match("tools", "tools"));
        assert!(glob_match("**/x*y/**", "a/b/xzzy/c"));
        assert!(glob_match("a*b*c", "abbbc"));
        assert!(!glob_match("a*b*c", "abbbcd"));
        assert!(glob_match("**", ""));
    }

    #[test]
    fn pnpm_yaml() {
        let y = "# c\npackages:\n  - 'packages/*'\n  - \"apps/*\" # apps\n  - '!**/test/**'\ncatalog:\n  - x\n";
        assert_eq!(pnpm_packages(y), ["packages/*", "apps/*", "!**/test/**"]);
        assert_eq!(pnpm_packages("packages: ['a/*', \"b\"]\n"), ["a/*", "b"]);
    }

    #[test]
    fn manifest_entries() {
        let m = json!({
            "types": "./dist/index.d.ts",
            "source": "./src/index.ts",
            "exports": {
                ".": { "types": "./dist/index.d.ts", "import": "./dist/index.mjs" },
                "./test": { "@acme/source": "./src/test/index.ts", "types": { "import": "./dist/test/index.d.mts", "require": "./dist/test/index.d.ts" } },
                "./package.json": "./package.json"
            }
        });
        let e = entries(&m);
        assert_eq!(
            e,
            vec![
                Entry {
                    types: "dist/index.d.ts".into(),
                    source: Some("src/index.ts".into())
                },
                Entry {
                    types: "dist/test/index.d.mts".into(),
                    source: Some("src/test/index.ts".into())
                },
                Entry {
                    types: "dist/test/index.d.ts".into(),
                    source: Some("src/test/index.ts".into())
                },
            ]
        );
        // Found by the `workspace` fuzz target: a pattern in `types` was taken as a file.
        assert!(entries(&json!({"types": "dist/*.d.ts"})).is_empty());
        let e = entries(&json!({"types": "a.d.ts", "source": "b.d.ts"}));
        assert_eq!(e[0].source, None);
        assert_eq!(normalize("./a/../b"), Some("b".into()));
        assert_eq!(normalize("../x"), None);
    }
}
