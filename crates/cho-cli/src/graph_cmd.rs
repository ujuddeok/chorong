//! `cho graph`: the reference graph from the cached semantic index, as JSON, DOT or a
//! self-contained HTML page. See `docs/public/graph.md`.

use crate::Output;
use anyhow::{Context, Result, anyhow, bail};
use cho_graph::{
    Changed, Direction, EdgeKind, Graph, GroupBy, Hunk, Orientation, PageOptions, Provenance,
    RefGraph, ViewSpec, blast_radius, changed_decls, diagram_from_graph, file_view, layout,
    neighbourhood, outbound_closure, package_view, resolve_symbol, to_dot, to_html, to_svg,
};
use cho_semantic::{IndexCache, Semantic};
use clap::Args;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Args)]
pub struct GraphArgs {
    /// Repository root (the folder `cho index` was run for).
    #[arg(long, default_value = ".")]
    root: PathBuf,
    /// A declaration: `NAME`, `Type::method`, `PACKAGE:NAME`, `FILE#NAME` or an exact SCIP symbol.
    /// Several matches are an error that lists them.
    #[arg(long, group = "target")]
    symbol: Option<String>,
    /// A file (repository-relative): its inbound and outbound edges, at package level.
    #[arg(long, group = "target")]
    file: Option<String>,
    /// A package (crate, npm or Python package): its boundary edges, at package level.
    #[arg(long, group = "target")]
    package: Option<String>,
    /// `BASE..HEAD`: the changed declarations of a local diff. Use with `--blast-radius`.
    #[arg(long, group = "target", requires = "blast_radius")]
    diff: Option<String>,
    /// `REPO N`: the blast radius of a pull request. Opens it as `cho open REPO --pr N` does
    /// and walks from its changes (merge base..head) in the worktree. Needs that worktree
    /// indexed (`cho index --root WORKTREE`).
    #[arg(long, group = "target", num_args = 2, value_names = ["REPO", "N"])]
    pr: Option<Vec<String>>,
    /// With `--diff`: walk inbound edges from the changed declarations and mark reachable tests.
    #[arg(long)]
    blast_radius: bool,
    /// With `--symbol`: everything it depends on, at package level (outbound, deep).
    #[arg(long, requires = "symbol")]
    closure: bool,
    /// Follow inbound edges (who uses it). With neither `--in` nor `--out`: both.
    #[arg(long = "in")]
    inbound: bool,
    /// Follow outbound edges (what it uses).
    #[arg(long = "out")]
    outbound: bool,
    /// Hops to walk. Default: 1 (both directions) or 2 (one direction) for a symbol, 1 for file
    /// and package, 3 for blast radius, 12 for closure.
    #[arg(long)]
    depth: Option<usize>,
    /// Only these edge kinds, e.g. `calls,implements`.
    #[arg(long, value_delimiter = ',', value_name = "KINDS")]
    kinds: Vec<String>,
    /// Declarations to keep before the rest become "+N more" nodes.
    #[arg(long, default_value_t = 60)]
    max_nodes: usize,
    /// Show third-party packages (one node each). Always on for closures, files and packages.
    #[arg(long)]
    externals: bool,
    /// Also draw `references` from code outside any declaration (imports, impl headers).
    #[arg(long)]
    file_level: bool,
    /// Repository reference used in `chorong://` links. Default: the worktree's remote
    /// (`origin`, else the only remote) as `github:`, `bitbucket:` or `bitbucket-dc:`, or
    /// `local:~/…` when it has none Chorong recognises. Never the absolute path under home.
    #[arg(long)]
    repo: Option<String>,
    /// Export for sharing: keep the repository and commit, drop the branch, the dirty state
    /// and the worktree name from links and the page header.
    #[arg(long)]
    share: bool,
    /// Print Graphviz DOT.
    #[arg(long)]
    dot: bool,
    /// Write a self-contained HTML page here.
    #[arg(long, value_name = "OUT")]
    html: Option<PathBuf>,
    /// Put the graph libraries inside the page (about 2 MB) so it works anywhere. Otherwise the
    /// page loads one shared copy from Chorong's cache folder by a relative path. `--share`
    /// implies it.
    #[arg(long, requires = "html")]
    inline: bool,
    /// Force the page theme (`dark` or `light`); default follows the system.
    #[arg(long, value_parser = ["dark", "light"])]
    theme: Option<String>,
    /// Write the graph as a static SVG diagram here (laid out by Chorong, the same drawing as the
    /// viewer's graph pane). Colours follow `--theme`, else the reader's light or dark setting.
    #[arg(long, value_name = "OUT")]
    svg: Option<PathBuf>,
    /// With `--svg`: draw a box around each file's or package's nodes.
    #[arg(long, requires = "svg", value_parser = ["none", "file", "package"], default_value = "none")]
    group: String,
    /// With `--svg`: edges run top to bottom instead of left to right.
    #[arg(long, requires = "svg")]
    top_down: bool,
    /// Open the HTML page in the default browser.
    #[arg(long, requires = "html")]
    open: bool,
}

pub fn run(mut a: GraphArgs) -> Result<Output> {
    let mut pr_notes = vec![];
    let mut pr_hunks = None;
    if let Some(pr) = a.pr.take() {
        let n = cho_sources::target::parse_pr_number(pr[1].trim_start_matches('#'))
            .map_err(|_| anyhow!("--pr expects REPO N, and `{}` is not a number", pr[1]))?;
        let opened = crate::sources_cmd::open_remote(
            &pr[0],
            &crate::sources_cmd::OpenRemote {
                pr: Some(n),
                ..Default::default()
            },
        )?;
        a.root = opened.worktree.clone();
        pr_hunks = Some(head_hunks(
            &opened.worktree,
            &opened.before.id,
            &opened.head.id,
        )?);
        a.diff = Some(format!("{}..{}", opened.before.id, opened.head.id));
        a.blast_radius = true;
        a.repo.get_or_insert(opened.repo.clone());
        pr_notes.push(format!(
            "Pull request {n}: Before {} ({}), After {}.",
            opened.before.short,
            match opened.base_source {
                cho_sources::BaseSource::MergeBase =>
                    format!("merge base with {}", opened.target_branch),
                cho_sources::BaseSource::Override => "override".into(),
            },
            opened.head.short
        ));
        pr_notes.extend(opened.notes);
    }
    if a.symbol.is_none() && a.file.is_none() && a.package.is_none() && a.diff.is_none() {
        bail!(
            "choose a starting point: --symbol, --file, --package, --diff BASE..HEAD --blast-radius or --pr REPO N"
        );
    }
    let root = a
        .root
        .canonicalize()
        .with_context(|| format!("cannot read {}", a.root.display()))?;
    let cache = IndexCache::default_location()
        .ok_or_else(|| anyhow!("cannot find a cache directory (HOME is not set)"))?;
    let sem = Semantic::open(&cache, &root)?.ok_or_else(|| {
        anyhow!(
            "no semantic index for {}; run `cho index --root {}` first",
            root.display(),
            a.root.display()
        )
    })?;
    let mut kinds = vec![];
    for k in &a.kinds {
        kinds.push(EdgeKind::parse(k).ok_or_else(|| {
            anyhow!(
                "unknown edge kind `{k}` (expected one of {})",
                EdgeKind::ALL.map(|k| k.as_str()).join(", ")
            )
        })?);
    }
    let git = cho_git::info(&root);
    let repo = match &a.repo {
        Some(r) => r.clone(),
        None => cho_git::repo_identity(&root).to_string(),
    };
    let repo_ref = cho_core::RepoRef::parse(&repo).map_err(|e| anyhow!("--repo: {e}"))?;
    if a.share {
        repo_ref.check_shareable().map_err(|e| anyhow!(e))?;
    }
    let mut provenance = Provenance {
        repo,
        worktree: display_worktree(&root),
        branch: git.branch.clone(),
        commit: git.commit.clone(),
        dirty: git.dirty,
        shared: false,
    };
    if a.share {
        provenance = provenance.for_sharing();
    }
    let direction = match (a.inbound, a.outbound) {
        (true, false) => Direction::In,
        (false, true) => Direction::Out,
        _ => Direction::Both,
    };
    let rg = RefGraph::build(&sem);
    let mut spec = ViewSpec::new(provenance);
    spec.direction = direction;
    spec.kinds = kinds;
    spec.max_nodes = a.max_nodes;
    spec.externals = a.externals;
    spec.file_level = a.file_level;

    let graph: Graph = if let Some(q) = &a.symbol {
        let ix = resolve_symbol(&rg, q).map_err(|e| anyhow!("{e}"))?;
        if a.closure {
            spec.depth = a.depth.unwrap_or(12);
            outbound_closure(&rg, &[ix], &spec)
        } else {
            spec.depth = a
                .depth
                .unwrap_or(if direction == Direction::Both { 1 } else { 2 });
            neighbourhood(&rg, &[ix], &spec)
        }
    } else if let Some(f) = &a.file {
        spec.depth = a.depth.unwrap_or(1);
        file_view(&rg, f.trim_start_matches("./"), &spec)
            .ok_or_else(|| anyhow!("`{f}` is not in the index"))?
    } else if let Some(p) = &a.package {
        spec.depth = a.depth.unwrap_or(1);
        package_view(&rg, p, &spec).ok_or_else(|| anyhow!("no package named `{p}` in the index"))?
    } else {
        let range = a.diff.as_deref().unwrap_or_default();
        let (base, head) = range
            .split_once("..")
            .filter(|(b, h)| !b.is_empty() && !h.is_empty() && !h.starts_with('.'))
            .ok_or_else(|| anyhow!("--diff expects BASE..HEAD"))?;
        let (hunks, removed) = match pr_hunks.take() {
            Some(h) => h,
            None => diff_hunks(&root, base, head)?,
        };
        let changed: Changed = changed_decls(&rg, &hunks);
        let mut notes = vec![];
        let head_commit = cho_git::rev_parse(&root, head);
        if head_commit.is_none() {
            bail!("`{head}` is not a commit in this repository");
        }
        if head_commit != git.full_commit {
            notes.push(format!(
                "The index and sources describe the checkout ({}), not {head}; changed lines were mapped onto the checkout and may be off.",
                git.commit.as_deref().unwrap_or("unknown")
            ));
        } else if git.dirty {
            notes.push("The checkout has uncommitted changes on top of the diff's head.".into());
        }
        spec.depth = a.depth.unwrap_or(3);
        notes.splice(0..0, pr_notes);
        blast_radius(&rg, &changed, &removed, &spec, notes)
    };

    let mut written = None;
    if let Some(out) = &a.html {
        let page = to_html(
            &graph,
            &PageOptions {
                theme: a.theme.as_deref().map(|t| match t {
                    "dark" => cho_theme::Appearance::Dark,
                    _ => cho_theme::Appearance::Light,
                }),
                assets: cho_graph::assets_for_page(out, a.share || a.inline),
            },
        );
        std::fs::write(out, page).with_context(|| format!("cannot write {}", out.display()))?;
        written = Some(out.clone());
        if a.open {
            open_in_browser(out);
        }
    }
    let mut svg_written = None;
    if let Some(out) = &a.svg {
        let group_by = match a.group.as_str() {
            "file" => GroupBy::File,
            "package" => GroupBy::Package,
            _ => GroupBy::None,
        };
        let orientation = if a.top_down {
            Orientation::TopDown
        } else {
            Orientation::LeftRight
        };
        let diagram = diagram_from_graph(&graph, group_by, orientation);
        let theme = a.theme.as_deref().map(|t| {
            cho_theme::Theme::bundled(match t {
                "dark" => cho_theme::Appearance::Dark,
                _ => cho_theme::Appearance::Light,
            })
        });
        let svg = to_svg(&diagram, &layout(&diagram), theme.as_ref());
        std::fs::write(out, svg).with_context(|| format!("cannot write {}", out.display()))?;
        svg_written = Some(out.clone());
    }
    let summary = format!(
        "{}: {} nodes, {} edges{}{}",
        graph.view.title,
        graph.nodes.len(),
        graph.edges.len(),
        if graph.view.truncated {
            " (capped)"
        } else {
            ""
        },
        written
            .iter()
            .chain(&svg_written)
            .map(|p| format!("; wrote {}", p.display()))
            .collect::<String>()
    );
    let text = if a.dot {
        to_dot(&graph)
    } else {
        summary_text(&graph, &summary)
    };
    let mut json = serde_json::to_value(&graph)?;
    if let Some(p) = written {
        json["html"] = json!(p.display().to_string());
    }
    if let Some(p) = svg_written {
        json["svg"] = json!(p.display().to_string());
    }
    Ok(Output::ok(text, json))
}

fn summary_text(g: &Graph, head: &str) -> String {
    let mut s = format!(
        "{head}\nindex {} · branch {} · commit {} · {}\n",
        g.meta.index_status,
        g.meta.provenance.branch.as_deref().unwrap_or("-"),
        g.meta.provenance.commit.as_deref().unwrap_or("-"),
        if g.meta.provenance.dirty {
            "dirty"
        } else {
            "clean"
        }
    );
    for e in &g.edges {
        s.push_str(&format!(
            "{} -> {} ({} x{})\n",
            short(&e.from),
            short(&e.to),
            e.kind.as_str(),
            e.count
        ));
    }
    for n in &g.notes {
        s.push_str(&format!("note: {n}\n"));
    }
    s
}

fn short(id: &str) -> String {
    match id.strip_prefix("sym:") {
        // `scheme manager package version descriptors`: keep package and descriptors.
        Some(sym) => {
            let mut p = sym.splitn(5, ' ');
            let (_, _, pkg, _, rest) = (p.next(), p.next(), p.next(), p.next(), p.next());
            format!("{}:{}", pkg.unwrap_or("?"), rest.unwrap_or(sym))
        }
        None => id.to_string(),
    }
}

/// The worktree as shown in the page: relative to where the command ran, or its folder
/// name. Never an absolute path.
fn display_worktree(root: &Path) -> String {
    let name = || {
        root.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "(worktree)".into())
    };
    let Ok(cwd) = std::env::current_dir().and_then(|c| c.canonicalize()) else {
        return name();
    };
    match root.strip_prefix(&cwd) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().into_owned(),
        _ => name(),
    }
}

/// Head-side hunks between two commits from `cho_changes` (read from the object store), and
/// the paths deleted. An empty head span (a pure deletion) covers the lines either side.
fn head_hunks(root: &Path, base: &str, head: &str) -> Result<(Vec<Hunk>, Vec<String>)> {
    let set = cho_changes::compare(root, base, head).map_err(|e| anyhow!(e))?;
    let (mut hunks, mut removed) = (vec![], vec![]);
    for f in set.files {
        if f.status == cho_changes::FileStatus::Deleted {
            removed.push(f.path);
            continue;
        }
        for h in f.hunks {
            let (start, count) = (h.head.start_line, h.head.line_count);
            let (start_line, end_line) = if count == 0 {
                (start.saturating_sub(1).max(1), start)
            } else {
                (start, start + count - 1)
            };
            hunks.push(Hunk {
                path: f.path.clone(),
                start_line,
                end_line,
            });
        }
    }
    Ok((hunks, removed))
}

/// Head-side hunks of `git diff -U0 BASE..HEAD`, and the paths it deletes.
fn diff_hunks(root: &Path, base: &str, head: &str) -> Result<(Vec<Hunk>, Vec<String>)> {
    let (hunks, removed) = cho_git::diff_hunks(root, base, head).map_err(|e| anyhow!(e))?;
    Ok((
        hunks
            .into_iter()
            .map(|h| Hunk {
                path: h.path,
                start_line: h.start_line,
                end_line: h.end_line,
            })
            .collect(),
        removed,
    ))
}

fn open_in_browser(path: &Path) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if Command::new(program).arg(path).spawn().is_err() {
        eprintln!(
            "could not run `{program}`; open {} yourself",
            path.display()
        );
    }
}
