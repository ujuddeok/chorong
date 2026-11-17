//! Show graph from the viewer: the same `cho-graph` engine as `cho graph`, run in-process
//! on the background executor, written as a self-contained HTML page to Chorong's cache
//! directory and opened in the default browser (`docs/public/graph.md`, "In the viewer").
//!
//! What the graph starts from is a [`Scope`]: a declaration (by its exact SCIP symbol, never
//! by name), a file, a package, or the changes on this branch (blast radius). The popover's
//! choices are [`GraphOptions`], remembered between uses in Chorong's settings.

use cho_graph::{
    Changed, Direction, EdgeKind, Graph, Hunk, PageOptions, Provenance, RefGraph, ViewSpec,
    blast_radius, changed_decls, file_view, neighbourhood, package_view, to_html,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Deepest walk the popover offers.
pub const MAX_DEPTH: usize = 6;

/// The popover's choices, kept in `graph.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphOptions {
    /// Stored as `in`, `out` or `both`; anything else reads as `both`.
    #[serde(deserialize_with = "direction_or_both")]
    pub direction: Direction,
    pub depth: usize,
    /// Edge kinds to keep, in [`EdgeKind::ALL`] order; empty means all. Unknown names are
    /// dropped on reading.
    #[serde(deserialize_with = "known_kinds")]
    pub kinds: Vec<EdgeKind>,
    /// Export for sharing: no branch, dirty state or worktree name in links and header.
    pub share: bool,
}

impl Default for GraphOptions {
    fn default() -> Self {
        Self {
            direction: Direction::Both,
            depth: 1,
            kinds: Vec::new(),
            share: false,
        }
    }
}

fn direction_or_both<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Direction, D::Error> {
    Ok(match String::deserialize(d)?.as_str() {
        "in" => Direction::In,
        "out" => Direction::Out,
        _ => Direction::Both,
    })
}

fn known_kinds<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<EdgeKind>, D::Error> {
    Ok(Vec::<String>::deserialize(d)?
        .iter()
        .filter_map(|k| EdgeKind::parse(k))
        .collect())
}

impl GraphOptions {
    /// Both, then who uses it, then what it uses.
    pub fn cycle_direction(&mut self) {
        self.direction = match self.direction {
            Direction::Both => Direction::In,
            Direction::In => Direction::Out,
            Direction::Out => Direction::Both,
        };
    }

    pub fn kind_on(&self, k: EdgeKind) -> bool {
        self.kinds.is_empty() || self.kinds.contains(&k)
    }

    /// Turns one edge kind off or on. Turning the last one off means all again.
    pub fn toggle_kind(&mut self, k: EdgeKind) {
        let on: Vec<EdgeKind> = EdgeKind::ALL
            .into_iter()
            .filter(|x| self.kind_on(*x) != (*x == k))
            .collect();
        self.kinds = if on.len() == EdgeKind::ALL.len() {
            Vec::new()
        } else {
            on
        };
    }

    pub fn load() -> Self {
        crate::prefs::file(FILE)
            .map(|p| crate::prefs::load(&p))
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        crate::prefs::save_named(FILE, self)
    }
}

const FILE: &str = "graph.json";

/// What the graph starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// A declaration, by its exact SCIP symbol; `label` is its display name.
    Symbol {
        symbol: String,
        label: String,
    },
    /// A repository-relative file (index paths).
    File {
        path: String,
    },
    Package {
        name: String,
    },
    /// The blast radius of the commits from `base` (a merge base) to `HEAD`.
    Branch {
        base: String,
        label: String,
    },
}

impl Scope {
    pub fn label(&self) -> String {
        match self {
            Scope::Symbol { label, .. } => format!("Symbol {label}"),
            Scope::File { path } => format!("File {path}"),
            Scope::Package { name } => format!("Package {name}"),
            Scope::Branch { label, .. } => format!("Changes on this branch ({label})"),
        }
    }

    fn slug(&self) -> String {
        let raw = match self {
            Scope::Symbol { label, .. } => format!("symbol-{label}"),
            Scope::File { path } => format!("file-{path}"),
            Scope::Package { name } => format!("package-{name}"),
            Scope::Branch { .. } => "blast-radius".to_string(),
        };
        let mut s: String = raw
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        s.truncate(60);
        s
    }
}

/// The comparison base for "Changes on this branch": the merge base of `HEAD` with the
/// remote's default branch (else `main`, else `master`). `None` when there is nothing to
/// compare (no such branch, or `HEAD` is on it).
pub fn branch_base(root: &Path) -> Option<(String, String)> {
    let candidates = [
        cho_git::git(
            root,
            &[
                "symbolic-ref",
                "--quiet",
                "--short",
                "refs/remotes/origin/HEAD",
            ],
        ),
        Some("main".into()),
        Some("master".into()),
    ];
    let head = cho_git::rev_parse(root, "HEAD")?;
    for name in candidates.into_iter().flatten() {
        if cho_git::rev_parse(root, &name).is_none() {
            continue;
        }
        let base = cho_git::git(root, &["merge-base", &name, "HEAD"])?;
        if base == head {
            return None;
        }
        return Some((base.clone(), format!("{name}..HEAD")));
    }
    None
}

/// A page made: where it was written and a one-line summary.
pub struct Built {
    pub path: PathBuf,
    pub summary: String,
}

/// Where pages go: `graphs/` beside the index cache.
pub fn out_dir() -> Option<PathBuf> {
    let cache = cho_semantic::IndexCache::default_location()?;
    let base = std::path::absolute(cache.base()).ok()?;
    Some(base.parent().unwrap_or(&base).join("graphs"))
}

/// Builds the view and writes the page. Runs on a background thread.
pub fn build(
    rg: &RefGraph,
    root: &Path,
    scope: &Scope,
    opts: &GraphOptions,
    dark: bool,
) -> Result<Built, String> {
    let git = cho_git::info(root);
    let repo = cho_git::repo_identity(root);
    if opts.share {
        repo.check_shareable()?;
    }
    let mut provenance = Provenance {
        repo: repo.to_string(),
        worktree: root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "(worktree)".into()),
        branch: git.branch.clone(),
        commit: git.commit.clone(),
        dirty: git.dirty,
        shared: false,
    };
    if opts.share {
        provenance = provenance.for_sharing();
    }
    let mut spec = ViewSpec::new(provenance);
    spec.direction = opts.direction;
    spec.depth = opts.depth.clamp(1, MAX_DEPTH);
    spec.kinds = opts.kinds.clone();
    let graph: Graph = match scope {
        Scope::Symbol { symbol, label } => {
            let ix = rg
                .decls
                .iter()
                .position(|d| d.symbol.symbol == *symbol)
                .ok_or_else(|| format!("{label} is not a declaration in the index"))?;
            neighbourhood(rg, &[ix as u32], &spec)
        }
        Scope::File { path } => {
            file_view(rg, path, &spec).ok_or_else(|| format!("{path} is not in the index"))?
        }
        Scope::Package { name } => package_view(rg, name, &spec)
            .ok_or_else(|| format!("no package named {name} in the index"))?,
        Scope::Branch { base, .. } => {
            let (hunks, removed) = cho_git::diff_hunks(root, base, "HEAD")?;
            let hunks: Vec<Hunk> = hunks
                .into_iter()
                .map(|h| Hunk {
                    path: h.path,
                    start_line: h.start_line,
                    end_line: h.end_line,
                })
                .collect();
            let changed: Changed = changed_decls(rg, &hunks);
            let mut notes = Vec::new();
            if git.dirty {
                notes.push(
                    "The checkout has uncommitted changes on top of the branch; they are not part of the diff.".into(),
                );
            }
            blast_radius(rg, &changed, &removed, &spec, notes)
        }
    };
    let dir = out_dir().ok_or("no cache directory (set HOME or CHORONG_CACHE_DIR)")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let name = format!(
        "{}-{}{}.html",
        scope.slug(),
        git.commit.as_deref().unwrap_or("nocommit"),
        if opts.share { "-shared" } else { "" }
    );
    let path = dir.join(name);
    let page = to_html(
        &graph,
        &PageOptions {
            theme: Some(if dark {
                cho_theme::Appearance::Dark
            } else {
                cho_theme::Appearance::Light
            }),
            assets: cho_graph::assets_for_page(&path, opts.share),
        },
    );
    std::fs::write(&path, page).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Built {
        summary: format!(
            "{}: {} nodes, {} edges{}",
            graph.view.title,
            graph.nodes.len(),
            graph.edges.len(),
            if graph.view.truncated {
                " (capped)"
            } else {
                ""
            }
        ),
        path,
    })
}

/// `path` with the home directory written as `~`, for display.
pub fn tilde(path: &Path) -> String {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty());
    match home.and_then(|h| path.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rel) => format!("~/{}", rel.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_cycle_and_toggle_kinds() {
        let mut o = GraphOptions::default();
        assert_eq!(o.direction, Direction::Both);
        o.cycle_direction();
        assert_eq!(o.direction, Direction::In);
        o.cycle_direction();
        o.cycle_direction();
        assert_eq!(o.direction, Direction::Both);
        assert!(EdgeKind::ALL.into_iter().all(|k| o.kind_on(k)));
        o.toggle_kind(EdgeKind::TypeUses);
        assert!(!o.kind_on(EdgeKind::TypeUses) && o.kind_on(EdgeKind::Calls));
        assert_eq!(o.kinds.len(), EdgeKind::ALL.len() - 1);
        o.toggle_kind(EdgeKind::TypeUses);
        assert!(o.kinds.is_empty(), "all on again is stored as all");
        let back: GraphOptions = serde_json::from_str(&serde_json::to_string(&o).unwrap()).unwrap();
        assert_eq!(back, o);
        let partial: GraphOptions = serde_json::from_str(r#"{"depth": 3}"#).unwrap();
        assert_eq!(partial.depth, 3);
        assert_eq!(partial.direction, Direction::Both);
        let old: GraphOptions =
            serde_json::from_str(r#"{"direction": "out", "kinds": ["calls", "gone"]}"#).unwrap();
        assert_eq!(
            (old.direction, old.kinds),
            (Direction::Out, vec![EdgeKind::Calls])
        );
    }

    #[test]
    fn page_names_are_safe() {
        let s = Scope::Symbol {
            symbol: "x".into(),
            label: "Queue::push<T>".into(),
        };
        assert_eq!(s.slug(), "symbol-Queue--push-T-");
    }
}
