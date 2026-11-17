//! `cho index` and `index status`: build the semantic (SCIP) index and report on it.

use crate::{IndexArgs, Output, index_cache, prepare_cmd, sources_cmd};
use anyhow::{Context, Result, anyhow};
use cho_semantic::{IndexStatus, Language as IndexLanguage};
use serde_json::{Value, json};
use std::path::Path;

pub fn index_build(args: &IndexArgs) -> Result<Output> {
    let root = args
        .root
        .canonicalize()
        .with_context(|| format!("cannot read {}", args.root.display()))?;
    // Chorong's own worktrees of remote projects need the project's approval to index.
    let note = prepare_cmd::gate_cache_worktree(&root)?;
    index_build_with(args, note)
}

/// Builds the index, recording a failed prepare (the index is then `partial`).
pub fn index_build_with(
    args: &IndexArgs,
    prepare: Option<cho_semantic::PrepareNote>,
) -> Result<Output> {
    let root = args
        .root
        .canonicalize()
        .with_context(|| format!("cannot read {}", args.root.display()))?;
    let only =
        match &args.only {
            Some(name) => Some(IndexLanguage::parse(name).ok_or_else(|| {
                anyhow!("unknown language `{name}`; use rust, typescript or python")
            })?),
            None => None,
        };
    let cache = index_cache()?;
    let projects = cho_semantic::indexer::detect(&root, only);
    if projects.is_empty() {
        return Ok(Output {
            text: "no indexable project found (Rust, TypeScript and Python are supported)".into(),
            json: json!({ "projects": [], "status": IndexStatus::None }),
            ok: false,
        });
    }
    let existing = cache.projects(&root);
    let mut text = String::new();
    let hints = node_modules_hints(&root, &projects);
    for h in &hints {
        text.push_str(&format!("hint: {h}\n"));
    }
    let mut results = vec![];
    let mut all_ok = true;
    // Ctrl-C (and SIGTERM) cancel the running indexer, whose process group is separate from
    // ours, and keep the previous index. The flag is checked every 25 ms.
    let interrupted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(sig, interrupted.clone()).context("cannot watch for Ctrl-C")?;
    }
    let mut store = cho_settings::SettingsStore::open_default();
    store.set_active_repo(Some(&root)).ok();
    let opts = cho_semantic::indexer::Options {
        timeout: std::time::Duration::from_secs(args.timeout),
        cancel: cho_semantic::indexer::CancelToken::from_flag(interrupted),
        node_memory_mb: store.effective().settings().index_node_memory_mb,
        prepare: None,
        tools: cho_sources::tools::config(store.effective().settings()),
        low_priority: store.effective().settings().index_low_priority,
    };
    for p in &projects {
        let slug = p.slug();
        let current = existing.iter().find(|e| {
            e.dir
                .file_name()
                .is_some_and(|n| n.to_string_lossy() == slug)
                && e.status == IndexStatus::Current
                && e.has_index
        });
        let label = format!("{} ({})", slug, if p.rel.is_empty() { "." } else { &p.rel });
        if current.is_some() && !args.force {
            text.push_str(&format!("{label}: up to date\n"));
            results.push(
                json!({ "project": slug, "result": "up-to-date", "status": IndexStatus::Current }),
            );
            continue;
        }
        let popts = cho_semantic::indexer::Options {
            prepare: prepare.clone(),
            ..opts.clone()
        };
        let mut reporter = ProgressLines::new(&slug);
        let refreshed = cho_semantic::indexer::refresh_with(&cache, &root, p, &popts, &mut |t| {
            reporter.tick(t)
        })?;
        let cho_semantic::indexer::Refresh::Done(meta) = refreshed else {
            text.push_str(&format!("{label}: cancelled; the previous index is kept\n"));
            results.push(json!({ "project": slug, "result": "cancelled" }));
            all_ok = false;
            break;
        };
        all_ok &= meta.success();
        if let (true, Some(n)) = (meta.success(), &meta.prepare) {
            text.push_str(&format!(
                "{label}: indexed in {:.1}s; partial (prepare {}; log {})\n",
                meta.duration_ms as f64 / 1000.0,
                n.result,
                n.log
            ));
        } else if meta.success() {
            text.push_str(&format!(
                "{label}: indexed in {:.1}s\n",
                meta.duration_ms as f64 / 1000.0
            ));
        } else {
            text.push_str(&format!(
                "{label}: FAILED{}\n",
                if meta.timed_out() { " (timed out)" } else { "" }
            ));
            for l in cho_semantic::indexer::report_lines(&meta.stderr_tail, 8) {
                text.push_str(&format!("    {l}\n"));
            }
        }
        results.push(json!({
            "project": slug,
            "result": if meta.success() { "indexed" } else { "failed" },
            "status": match (meta.success(), &meta.prepare) {
                (false, _) => IndexStatus::Failed,
                (true, Some(_)) => IndexStatus::Partial,
                (true, None) => IndexStatus::Current,
            },
            "meta": meta,
        }));
    }
    Ok(Output {
        text,
        json: json!({ "root": root, "projects": results, "hints": hints }),
        ok: all_ok,
    })
}

/// Progress lines on stderr while `cho index` builds: one when the indexer's step changes and a
/// heartbeat every ten seconds, each with the elapsed time (stdout stays the result only).
struct ProgressLines<'a> {
    project: &'a str,
    last_step: Option<String>,
    last_print: Option<std::time::Instant>,
}

impl<'a> ProgressLines<'a> {
    fn new(project: &'a str) -> Self {
        Self {
            project,
            last_step: None,
            last_print: None,
        }
    }

    fn tick(&mut self, t: &cho_semantic::indexer::Tick) {
        let changed = t.progress.step != self.last_step;
        let due = self
            .last_print
            .is_none_or(|p| p.elapsed() >= std::time::Duration::from_secs(10));
        if changed || due {
            self.last_step = t.progress.step.clone();
            self.last_print = Some(std::time::Instant::now());
            eprintln!(
                "{}",
                cho_semantic::progress::describe("indexing", self.project, &t.progress, t.elapsed)
            );
        }
    }
}

/// TypeScript projects that need `node_modules` and have none (`cho_semantic::node_modules`,
/// the rule the viewer uses too): what to run first. In a user's own folder Chorong never
/// installs anything, so the hint names the command; in Chorong's own worktrees it names
/// `cho prepare`.
fn node_modules_hints(root: &Path, projects: &[cho_semantic::Project]) -> Vec<String> {
    use cho_semantic::node_modules::{self, NodeModules};
    let in_cache = sources_cmd::cache()
        .ok()
        .and_then(|c| c.worktree_owner(root));
    let mut out = vec![];
    for p in projects {
        let NodeModules::Missing { at } = node_modules::check(p, root) else {
            continue;
        };
        if let Some((repo, _)) = &in_cache {
            out.push(format!(
                "{at} has no node_modules; prepare the worktree first: `cho prepare run {repo}`"
            ));
            continue;
        }
        let files: Vec<String> = [&p.root, &root.to_path_buf()]
            .iter()
            .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let names: Vec<&str> = files.iter().map(String::as_str).collect();
        let cmd = cho_sources::prepare::suggestion(&names)
            .and_then(|(r, _)| {
                r.steps
                    .iter()
                    .find(|s| s.argv[0] != "uv")
                    .map(|s| s.display())
            })
            .unwrap_or_else(|| "npm install".into());
        out.push(format!(
            "{at} has no node_modules, so TypeScript types from packages won't resolve; run `{cmd}` there first (Chorong doesn't install packages in your folders)"
        ));
    }
    out
}

pub fn index_status(root: &Path) -> Result<Output> {
    let root = root
        .canonicalize()
        .with_context(|| format!("cannot read {}", root.display()))?;
    let projects = index_cache()?.projects(&root);
    let overall = cho_semantic::cache::overall_status(&projects);
    let mut text = format!("{overall}\n");
    let list: Vec<Value> = projects
        .iter()
        .map(|p| {
            text.push_str(&format!(
                "  {:<24} {:<8} {} {}\n",
                p.dir.file_name().unwrap_or_default().to_string_lossy(),
                p.status,
                p.meta.indexer,
                p.meta.indexer_version.as_deref().unwrap_or("")
            ));
            if p.status == IndexStatus::Failed {
                for l in cho_semantic::indexer::report_lines(&p.meta.stderr_tail, 5) {
                    text.push_str(&format!("      {l}\n"));
                }
            }
            json!({
                "project": p.dir.file_name().unwrap_or_default().to_string_lossy(),
                "status": p.status,
                "has_index": p.has_index,
                "meta": p.meta,
            })
        })
        .collect();
    Ok(Output {
        text,
        json: json!({ "root": root, "status": overall, "projects": list }),
        ok: matches!(
            overall,
            IndexStatus::Current | IndexStatus::Stale | IndexStatus::Partial
        ),
    })
}

#[cfg(test)]
mod node_modules_tests {
    use super::*;

    /// A hoisted `node_modules` at the repository root covers every package of a workspace,
    /// and a package that declares no dependencies needs none: neither gets a hint.
    #[test]
    #[allow(clippy::disallowed_methods)] // the test writes its own files
    fn hints_only_for_missing_dependencies() {
        let dir = std::env::temp_dir().join(format!("cho-cli-nm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.canonicalize().unwrap();
        let web = root.join("web");
        std::fs::create_dir_all(&web).unwrap();
        std::fs::write(web.join("package.json"), r#"{"dependencies":{"x":"1"}}"#).unwrap();
        let projects = [cho_semantic::Project {
            language: IndexLanguage::TypeScript,
            root: web.clone(),
            rel: "web".into(),
        }];
        let hints = node_modules_hints(&root, &projects);
        assert!(
            hints.len() == 1 && hints[0].starts_with("web has no node_modules"),
            "{hints:?}"
        );
        std::fs::create_dir(root.join("node_modules")).unwrap();
        assert!(node_modules_hints(&root, &projects).is_empty());
        std::fs::remove_dir(root.join("node_modules")).unwrap();
        std::fs::write(web.join("package.json"), "{}").unwrap();
        assert!(node_modules_hints(&root, &projects).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
