//! Joins a workspace package's built declarations (`dist/*.d.ts`) to its source.
//!
//! In a monorepo whose packages export `dist`, scip-typescript gives every use from
//! another package the symbol of the built declaration
//! (`… npm @acme/core 1.0.0 dist/`x.d.ts`/Thing#`), which has no definition in the index,
//! while the source declaration has its own symbol (`… src/`x.ts`/Thing#`). This module
//! links the two, once per index load:
//!
//! - the symbol's package name and version must equal a workspace package's, its file
//!   must be a declaration file under that package's output directory, and the symbol
//!   must have no definition in the index;
//! - a source file is proposed by, in order of strength, the declaration map
//!   (`x.d.ts.map`), the tsconfig `outDir`/`declarationDir` to `rootDir` mapping, the
//!   manifest's `source` field or source export condition for an entry point, and a
//!   `dist` to `src` path swap;
//! - the link is made only if the index has a **definition** of the symbol with the same
//!   package, version and descriptor chain in that file. A bundled declaration (one
//!   `index.d.ts` for the whole package) is followed through the source entry's
//!   `export … from` statements to the file that defines the name, and marked as such.
//!
//! Nothing is linked on a name alone, and every link is labelled "mapped (unverified)"
//! with its method: the index did not record it.

use crate::index::{ScipIndex, SymbolId};
use crate::relpath::{below, join, normalize, parent};
use crate::symbol::Header;
use crate::workspace::{
    self, DECLARATION_EXTS, WorkspacePackage, is_declaration_file, source_names,
};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How a built declaration was mapped to its source, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MapMethod {
    /// The declaration map (`x.d.ts.map`) names the source file.
    DeclarationMap,
    /// tsconfig `outDir` (or `declarationDir`) to `rootDir`.
    Tsconfig,
    /// The manifest's `source` field or a source export condition, for an entry point.
    SourceField,
    /// `dist/…` to `src/…`, with nothing better to go on.
    PathSwap,
}

impl MapMethod {
    pub const ALL: [MapMethod; 4] = [
        Self::DeclarationMap,
        Self::Tsconfig,
        Self::SourceField,
        Self::PathSwap,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeclarationMap => "declaration map",
            Self::Tsconfig => "tsconfig",
            Self::SourceField => "source field",
            Self::PathSwap => "path swap",
        }
    }
}

/// Why two symbols are linked. Never verified by the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Mapping {
    pub method: MapMethod,
    /// Found through the source entry's re-exports (a bundled declaration file): weaker
    /// than a file-to-file mapping.
    pub through_reexports: bool,
}

impl Mapping {
    /// `mapped (unverified): declaration map`, `mapped (unverified): path swap, through re-exports`.
    pub fn label(&self) -> String {
        format!(
            "mapped (unverified): {}{}",
            self.method.as_str(),
            if self.through_reexports {
                ", through re-exports"
            } else {
                ""
            }
        )
    }
}

/// What the linking found and what it cost (the `dist_links` example prints it for the docs).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LinkStats {
    /// Workspace packages with indexed documents.
    pub packages: usize,
    /// Symbols of a workspace package in one of its declaration files, with no definition.
    pub candidates: usize,
    /// Of those, linked to a source definition.
    pub mapped: usize,
    /// Linked, by method (`declaration-map`, …) and whether through re-exports.
    pub by_method: Vec<(MapMethod, bool, usize)>,
    #[serde(serialize_with = "millis")]
    pub elapsed: Duration,
}

fn millis<S: serde::Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(d.as_millis() as u64)
}

/// The links of one loaded index: each built-declaration symbol to its source symbol.
#[derive(Debug, Default)]
pub struct DistLinks {
    to_source: HashMap<SymbolId, (SymbolId, Mapping)>,
    from_source: HashMap<SymbolId, Vec<SymbolId>>,
    stats: LinkStats,
}

impl DistLinks {
    /// The source symbol a built-declaration symbol maps to.
    pub fn source_of(&self, id: &SymbolId) -> Option<(&SymbolId, Mapping)> {
        self.to_source.get(id).map(|(s, m)| (s, *m))
    }

    /// The built-declaration symbols that map to `source`, sorted.
    pub fn mapped_to(&self, source: &SymbolId) -> &[SymbolId] {
        self.from_source
            .get(source)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn stats(&self) -> &LinkStats {
        &self.stats
    }

    pub fn is_empty(&self) -> bool {
        self.to_source.is_empty()
    }

    /// Every link, `(built, source, mapping)`, in symbol order.
    pub fn links(&self) -> Vec<(&SymbolId, &SymbolId, Mapping)> {
        let mut v: Vec<_> = self
            .to_source
            .iter()
            .map(|(d, (s, m))| (d, s, *m))
            .collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        v
    }

    /// Links the index's built-declaration symbols for the repository at `root`.
    /// `project_roots` are the indexed projects (repository-relative), where workspace
    /// files are looked for besides the root.
    pub fn compute(index: &ScipIndex, root: &Path, project_roots: &[String]) -> Self {
        let started = Instant::now();
        let mut out = Self::default();
        let doc_dirs: BTreeSet<String> = index
            .documents()
            .iter()
            .map(|d| parent(&d.path).to_string())
            .collect();
        let packages = workspace::detect(root, project_roots, &doc_dirs);
        out.stats.packages = packages.len();
        if packages.is_empty() {
            out.stats.elapsed = started.elapsed();
            return out;
        }
        let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, p) in packages.iter().enumerate() {
            by_name.entry(p.name.as_str()).or_default().push(i);
        }
        let mut linker = Linker {
            index,
            root,
            defined: HashMap::new(),
            maps: HashMap::new(),
            exports: HashMap::new(),
            reached: HashMap::new(),
        };
        let out_dirs: Vec<Vec<String>> = packages.iter().map(|p| p.output_dirs()).collect();
        // Pass 1, over every symbol: split only the package header (no allocation), keep
        // the defined symbols of workspace packages for later and fully parse only the
        // undefined ones named in a declaration file.
        let mut candidates: Vec<(usize, &SymbolId, Parsed<'_>)> = vec![];
        let mut defined: Vec<(usize, &SymbolId)> = vec![];
        for (id, occs) in index.symbols() {
            if id.is_local() {
                continue;
            }
            let Some((name, version, desc)) = npm_header(&id.symbol) else {
                continue;
            };
            let Some(ixs) = by_name.get(name) else {
                continue;
            };
            let mut owners = ixs
                .iter()
                .copied()
                .filter(|&i| packages[i].version_matches(version));
            let (Some(pkg), None) = (owners.next(), owners.next()) else {
                continue; // no workspace package, or two with the same name and version
            };
            if !occs.definitions.is_empty() {
                defined.push((pkg, id));
                continue;
            }
            if !names_declaration_file(desc) {
                continue;
            }
            let Some(p) = parse_symbol(&id.symbol) else {
                continue;
            };
            if is_declaration_file(&p.file)
                && out_dirs[pkg].iter().any(|o| below(&p.file, o).is_some())
            {
                candidates.push((pkg, id, p));
            }
        }
        // Pass 2: the definitions of the packages that have candidates, by file and name chain.
        let wanted: HashSet<usize> = candidates.iter().map(|c| c.0).collect();
        for (pkg, id) in defined {
            if !wanted.contains(&pkg) {
                continue;
            }
            if let Some(p) = parse_symbol(&id.symbol) {
                linker
                    .defined
                    .entry(pkg)
                    .or_default()
                    .entry(p.file)
                    .or_default()
                    .entry(p.rest.to_string())
                    .or_default()
                    .push(id.clone());
            }
        }
        out.stats.candidates = candidates.len();
        let mut by_method: HashMap<(MapMethod, bool), usize> = HashMap::new();
        for (pkg, id, p) in candidates {
            if let Some((source, mapping)) = linker.link(&packages[pkg], pkg, &p) {
                *by_method
                    .entry((mapping.method, mapping.through_reexports))
                    .or_default() += 1;
                out.from_source
                    .entry(source.clone())
                    .or_default()
                    .push(id.clone());
                out.to_source.insert(id.clone(), (source, mapping));
            }
        }
        for v in out.from_source.values_mut() {
            v.sort();
        }
        out.stats.mapped = out.to_source.len();
        let mut by: Vec<_> = by_method.into_iter().map(|((m, r), n)| (m, r, n)).collect();
        by.sort();
        out.stats.by_method = by;
        out.stats.elapsed = started.elapsed();
        out
    }
}

/// A SCIP npm symbol split into its package, the file its descriptors start with, and
/// the descriptors after the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed<'a> {
    pub name: &'a str,
    pub version: &'a str,
    /// Package-relative, decoded (`dist/x.d.ts`).
    pub file: String,
    /// The descriptors after the file, as written (`Thing#method().`); empty for the module.
    pub rest: &'a str,
}

const SOURCE_EXTS: [&str; 8] = [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"];

/// `(name, version, descriptors)` of a `scheme npm name version descriptors` symbol.
fn npm_header(symbol: &str) -> Option<(&str, &str, &str)> {
    let h = Header::parse(symbol).filter(|h| h.manager == "npm")?;
    Some((h.name, h.version, h.descriptors))
}

/// Do the descriptors name a declaration file (`` `x.d.ts`/ ``)? Allocation-free: it runs
/// over every symbol of the index.
fn names_declaration_file(desc: &str) -> bool {
    DECLARATION_EXTS.iter().any(|(ext, _)| {
        desc.match_indices(ext)
            .any(|(i, _)| desc[i + ext.len()..].starts_with("`/"))
    })
}

/// Splits `scheme npm name version descriptors`. The file is the leading run of
/// namespace descriptors (`dist/`, `` `x.d.ts`/ ``) up to the first one named like a
/// script file. `None` for other managers or when no file descriptor leads.
pub fn parse_symbol(symbol: &str) -> Option<Parsed<'_>> {
    let h = Header::parse(symbol).filter(|h| h.manager == "npm")?;
    let (name, version, desc) = (h.package()?, h.version, h.descriptors);
    let mut segs: Vec<String> = vec![];
    let mut rest = desc;
    loop {
        let (seg, after) = descriptor_name(rest)?;
        let after = after.strip_prefix('/')?;
        let is_file = SOURCE_EXTS.iter().any(|e| seg.ends_with(e));
        segs.push(seg);
        rest = after;
        if is_file {
            break;
        }
    }
    Some(Parsed {
        name,
        version,
        file: segs.join("/"),
        rest,
    })
}

/// The name at the start of a descriptor string (simple or `` `escaped` ``) and what follows.
fn descriptor_name(s: &str) -> Option<(String, &str)> {
    if let Some(body) = s.strip_prefix('`') {
        let mut name = String::new();
        let mut chars = body.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c == '`' {
                if chars.peek().map(|&(_, c)| c) == Some('`') {
                    chars.next();
                    name.push('`');
                } else {
                    return Some((name, &body[i + 1..]));
                }
            } else {
                name.push(c);
            }
        }
        None
    } else {
        let end = s
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-' | '$')))
            .unwrap_or(s.len());
        (end > 0).then(|| (s[..end].to_string(), &s[end..]))
    }
}

/// The first descriptor's name and its whole text with its suffix: `User#name.` gives
/// `("User", "User#")`, `parse().(text)` gives `("parse", "parse().")`. `None` for a module
/// (empty) or an unusual first descriptor.
fn top_level(rest: &str) -> Option<(String, &str)> {
    let (name, after) = descriptor_name(rest)?;
    let at = rest.len() - after.len();
    let len = match after.as_bytes().first()? {
        b'#' | b'.' => 1,
        b'(' => after.find(").")? + 2,
        _ => return None,
    };
    Some((name, &rest[..at + len]))
}

/// One `export … from '…'` statement: the names it re-exports as `(local, exported)`, or
/// `None` for `export * from`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReExport {
    pub names: Option<Vec<(String, String)>>,
    pub from: String,
}

/// The one symbol `hits` name: `Some(None)` for none, `None` when they name two or more
/// (no link: the engine never picks one).
fn unique<'s>(hits: impl Iterator<Item = &'s SymbolId>) -> Option<Option<SymbolId>> {
    let mut hits: Vec<&SymbolId> = hits.collect();
    hits.sort();
    hits.dedup();
    match hits[..] {
        [] => Some(None),
        [one] => Some(Some(one.clone())),
        _ => None,
    }
}

struct Linker<'a> {
    index: &'a ScipIndex,
    root: &'a Path,
    /// Package, file, descriptors to the symbols defined there (one, normally). Nested so
    /// lookups borrow their keys.
    defined: HashMap<usize, HashMap<String, HashMap<String, Vec<SymbolId>>>>,
    /// Declaration map sources per repository-relative declaration file.
    maps: HashMap<String, Rc<Vec<String>>>,
    /// Re-exports per repository-relative source file.
    exports: HashMap<String, Rc<Vec<ReExport>>>,
    /// Files a re-export walk ended at, per package, method, declaration file and top-level
    /// declaration: every member of one declaration shares the walk.
    reached: HashMap<(usize, MapMethod, String, String), Rc<Vec<String>>>,
}

impl Linker<'_> {
    fn link(
        &mut self,
        pkg: &WorkspacePackage,
        pkg_ix: usize,
        p: &Parsed<'_>,
    ) -> Option<(SymbolId, Mapping)> {
        let proposals: Vec<(MapMethod, Rc<Vec<String>>)> = MapMethod::ALL
            .iter()
            .map(|&m| (m, self.propose(pkg, m, &p.file)))
            .collect();
        // File to file first, strongest method first. Two different answers: no link.
        for (method, files) in &proposals {
            let hits = files
                .iter()
                .filter_map(|f| self.defined_at(pkg_ix, f, p.rest))
                .flatten();
            if let Some(one) = unique(hits)? {
                return Some((
                    one,
                    Mapping {
                        method: *method,
                        through_reexports: false,
                    },
                ));
            }
        }
        // A bundled declaration: follow the proposed source files' re-exports to the file
        // that defines the top-level declaration, then check the whole chain there.
        let (name, head) = top_level(p.rest)?;
        for (method, files) in &proposals {
            let key = (pkg_ix, *method, p.file.clone(), head.to_string());
            let ends = match self.reached.get(&key) {
                Some(e) => e.clone(),
                None => {
                    // One visited set per method: a file reached twice adds nothing.
                    let mut seen = HashSet::new();
                    let mut ends = vec![];
                    for f in files.iter() {
                        self.through_exports(pkg, pkg_ix, f, &name, head, 0, &mut seen, &mut ends);
                    }
                    ends.sort();
                    ends.dedup();
                    let ends = Rc::new(ends);
                    self.reached.insert(key, ends.clone());
                    ends
                }
            };
            let hits = ends
                .iter()
                .filter_map(|f| self.defined_at(pkg_ix, f, p.rest))
                .flatten();
            if let Some(one) = unique(hits)? {
                return Some((
                    one,
                    Mapping {
                        method: *method,
                        through_reexports: true,
                    },
                ));
            }
        }
        None
    }

    /// The symbols defined in `file` (package-relative) of package `pkg` with the
    /// descriptors `rest`.
    fn defined_at(&self, pkg: usize, file: &str, rest: &str) -> Option<&Vec<SymbolId>> {
        self.defined.get(&pkg)?.get(file)?.get(rest)
    }

    /// Source files (package-relative) that `method` proposes for the declaration `file`.
    fn propose(
        &mut self,
        pkg: &WorkspacePackage,
        method: MapMethod,
        file: &str,
    ) -> Rc<Vec<String>> {
        Rc::new(match method {
            MapMethod::DeclarationMap => return self.declaration_map(pkg, file),
            MapMethod::Tsconfig => {
                let Some(root_dir) = &pkg.tsconfig.root_dir else {
                    return Rc::default();
                };
                let out = [&pkg.tsconfig.declaration_dir, &pkg.tsconfig.out_dir]
                    .into_iter()
                    .flatten()
                    // An outDir that is the package itself maps nothing.
                    .filter(|o| !o.is_empty())
                    .find_map(|o| below(file, o));
                out.map(|rel| {
                    source_names(rel)
                        .into_iter()
                        .filter_map(|s| normalize(&join(root_dir, &s)))
                        .collect()
                })
                .unwrap_or_default()
            }
            MapMethod::SourceField => pkg
                .entries
                .iter()
                .filter(|e| e.types == file)
                .filter_map(|e| e.source.clone())
                .collect(),
            MapMethod::PathSwap => pkg
                .output_dirs()
                .iter()
                .find_map(|o| below(file, o))
                .map(|rel| {
                    source_names(rel)
                        .into_iter()
                        .map(|s| format!("src/{s}"))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// The source files `file.map` names, package-relative. Files outside the package
    /// are dropped.
    fn declaration_map(&mut self, pkg: &WorkspacePackage, file: &str) -> Rc<Vec<String>> {
        let map_path = join(&pkg.dir, &format!("{file}.map"));
        if let Some(hit) = self.maps.get(&map_path) {
            return hit.clone();
        }
        let found = cho_core::fs::read_source_text_in(self.root, Path::new(&map_path))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .map(|v| {
                let source_root = v
                    .get("sourceRoot")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string();
                let dir = parent(file);
                v.get("sources")
                    .and_then(|s| s.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| s.as_str())
                            .filter(|s| !s.contains("://"))
                            .filter_map(|s| normalize(&join(&join(dir, &source_root), s)))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let found = Rc::new(found);
        self.maps.insert(map_path, found.clone());
        found
    }

    /// Source files reached from `file` through `export … from` statements that define the
    /// top-level declaration `head` (`Name#`, `name().`), into `ends`. A renamed re-export
    /// (`export { a as b }`) is not followed: its descriptors differ, so the definition can't
    /// be checked.
    #[allow(clippy::too_many_arguments)]
    fn through_exports(
        &mut self,
        pkg: &WorkspacePackage,
        pkg_ix: usize,
        file: &str,
        name: &str,
        head: &str,
        depth: usize,
        seen: &mut HashSet<String>,
        ends: &mut Vec<String>,
    ) {
        if depth > 16 || !seen.insert(file.to_string()) {
            return;
        }
        if self.defined_at(pkg_ix, file, head).is_some() {
            ends.push(file.to_string());
            return;
        }
        let exports = self.exports_of(&join(&pkg.dir, file));
        let explicit = exports.iter().any(|e| {
            e.names
                .as_ref()
                .is_some_and(|n| n.iter().any(|(_, exported)| exported == name))
        });
        let follow: Vec<&String> = exports
            .iter()
            .filter(|e| match &e.names {
                None => !explicit,
                Some(n) => explicit && n.iter().any(|(l, x)| x == name && l == name),
            })
            .map(|e| &e.from)
            .collect();
        for spec in follow {
            if let Some(target) = self.resolve_module(pkg, file, spec) {
                self.through_exports(pkg, pkg_ix, &target, name, head, depth + 1, seen, ends);
            }
        }
    }

    fn exports_of(&mut self, repo_path: &str) -> Rc<Vec<ReExport>> {
        if let Some(hit) = self.exports.get(repo_path) {
            return hit.clone();
        }
        let found = cho_core::fs::read_source_text_in(self.root, Path::new(repo_path))
            .map(|t| Rc::new(reexports(&t)))
            .unwrap_or_default();
        self.exports.insert(repo_path.to_string(), found.clone());
        found
    }

    /// A relative module specifier from `from` (package-relative) to an indexed source
    /// file of the same package.
    fn resolve_module(&self, pkg: &WorkspacePackage, from: &str, spec: &str) -> Option<String> {
        if !spec.starts_with("./") && !spec.starts_with("../") {
            return None;
        }
        let base = normalize(&join(parent(from), spec))?;
        let stem_of = |ext: &str| base.strip_suffix(ext).map(str::to_string);
        let candidates: Vec<String> = if [".ts", ".tsx", ".mts", ".cts"]
            .iter()
            .any(|e| base.ends_with(e))
        {
            vec![base.clone()]
        } else if let Some(s) = stem_of(".js") {
            vec![format!("{s}.ts"), format!("{s}.tsx")]
        } else if let Some(s) = stem_of(".jsx") {
            vec![format!("{s}.tsx")]
        } else if let Some(s) = stem_of(".mjs") {
            vec![format!("{s}.mts")]
        } else if let Some(s) = stem_of(".cjs") {
            vec![format!("{s}.cts")]
        } else {
            vec![
                format!("{base}.ts"),
                format!("{base}.tsx"),
                format!("{base}/index.ts"),
                format!("{base}/index.tsx"),
            ]
        };
        candidates
            .into_iter()
            .find(|c| !self.index.documents_for(&join(&pkg.dir, c)).is_empty())
    }
}

/// The `export * from '…'` and `export { a, b as c } from '…'` statements of a module
/// (`type` modifiers allowed). `export * as ns from` is skipped: the names behind a
/// namespace are not the module's own exports.
pub fn reexports(text: &str) -> Vec<ReExport> {
    let code = strip_comments(text);
    let b = code.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    let word_at = |i: usize, w: &str| {
        code[i..].starts_with(w)
            && (i == 0 || !is_ident(b[i - 1]))
            && b.get(i + w.len()).is_none_or(|&c| !is_ident(c))
    };
    let skip_ws = |mut i: usize| {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        i
    };
    while let Some(off) = code[i..].find("export") {
        let at = i + off;
        i = at + 6;
        if !word_at(at, "export") {
            continue;
        }
        let mut j = skip_ws(i);
        if word_at(j, "type") {
            j = skip_ws(j + 4);
        }
        let names = match b.get(j) {
            Some(b'*') => {
                j = skip_ws(j + 1);
                if word_at(j, "as") {
                    continue;
                }
                None
            }
            Some(b'{') => {
                let Some(close) = code[j..].find('}') else {
                    break;
                };
                let list = &code[j + 1..j + close];
                j = skip_ws(j + close + 1);
                Some(
                    list.split(',')
                        .filter_map(|item| {
                            let words: Vec<&str> = item.split_whitespace().collect();
                            let words = match words.first() {
                                Some(&"type") if words.len() > 1 => &words[1..],
                                _ => &words[..],
                            };
                            match words {
                                [n] => Some((n.to_string(), n.to_string())),
                                [l, "as", x] => Some((l.to_string(), x.to_string())),
                                _ => None,
                            }
                        })
                        .collect(),
                )
            }
            _ => continue,
        };
        if !word_at(j, "from") {
            continue;
        }
        j = skip_ws(j + 4);
        let Some(&q) = b.get(j).filter(|&&q| q == b'\'' || q == b'"') else {
            continue;
        };
        let Some(end) = code[j + 1..].find(q as char) else {
            break;
        };
        out.push(ReExport {
            names,
            from: code[j + 1..j + 1 + end].to_string(),
        });
        i = j + 1 + end;
    }
    out
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// The text with `//` and `/* */` comments blanked; string contents kept.
fn strip_comments(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            q @ (b'"' | b'\'' | b'`') => {
                let start = i;
                i += 1;
                while i < b.len() && b[i] != q && !(q != b'`' && b[i] == b'\n') {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(b.len());
                out.extend_from_slice(&b[start..i]);
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(b.len());
                out.push(b' ');
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_npm_symbols() {
        let p = parse_symbol(
            "scip-typescript npm @ai-sdk/provider 4.0.35 dist/language-model/v4/`language-model-v4.d.ts`/LanguageModelV4#",
        )
        .unwrap();
        assert_eq!(p.name, "@ai-sdk/provider");
        assert_eq!(p.version, "4.0.35");
        assert_eq!(p.file, "dist/language-model/v4/language-model-v4.d.ts");
        assert_eq!(p.rest, "LanguageModelV4#");
        let m = parse_symbol("scip-typescript npm web HEAD src/`m.ts`/add().").unwrap();
        assert_eq!((m.file.as_str(), m.rest), ("src/m.ts", "add()."));
        let module = parse_symbol("scip-typescript npm web HEAD src/`m.ts`/").unwrap();
        assert_eq!(module.rest, "");
        let tick = parse_symbol("scip-typescript npm w 1 `a``b.ts`/x.").unwrap();
        assert_eq!(tick.file, "a`b.ts");
        assert!(parse_symbol("rust-analyzer cargo p 0.1 a/foo().").is_none());
        assert!(parse_symbol("scip-typescript npm w 1 src/Foo#").is_none());
        assert!(parse_symbol("local 3").is_none());
    }

    #[test]
    fn reads_reexports() {
        let src = "// export * from './no'\nexport * from './a';\nexport type * from \"./b.js\";\nexport * as ns from './c';\nexport { x, y as z, type T } from './d';\nexport type { U } from './e';\nexport { local };\nexport const q = 1; /* export * from './no2' */\n";
        let r = reexports(src);
        let froms: Vec<&str> = r.iter().map(|e| e.from.as_str()).collect();
        assert_eq!(froms, ["./a", "./b.js", "./d", "./e"]);
        assert_eq!(r[0].names, None);
        assert_eq!(
            r[2].names,
            Some(vec![
                ("x".into(), "x".into()),
                ("y".into(), "z".into()),
                ("T".into(), "T".into())
            ])
        );
    }

    #[test]
    fn top_level_declarations() {
        assert_eq!(top_level("User#name."), Some(("User".into(), "User#")));
        assert_eq!(
            top_level("parse().(text)"),
            Some(("parse".into(), "parse()."))
        );
        assert_eq!(top_level("Missing."), Some(("Missing".into(), "Missing.")));
        assert_eq!(top_level(""), None);
    }

    #[test]
    fn labels() {
        let m = Mapping {
            method: MapMethod::PathSwap,
            through_reexports: true,
        };
        assert_eq!(
            m.label(),
            "mapped (unverified): path swap, through re-exports"
        );
        assert_eq!(source_names("a/b.d.ts"), ["a/b.ts", "a/b.tsx"]);
        assert_eq!(source_names("a/b.d.mts"), ["a/b.mts"]);
    }
}
