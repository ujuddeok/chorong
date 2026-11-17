//! `cho investigation validate|check|list`.

use crate::{Output, open_index};
use anyhow::{Context, Result, bail};
use cho_investigation::check::{EdgeOracle, NodeStatus, WorktreeReader, check_with_reader};
use cho_investigation::check_v2::{AnchorReport, PaneReport, ResultSource, RowReport, V2Report};
use cho_investigation::drift::{CommitReader, Drift, drift};
use cho_investigation::listing::{self, EntryInfo};
use cho_investigation::source::{self, Source};
use cho_investigation::{Evidence, Investigation, Kind, RowResult, Verification};
use cho_semantic::IndexStatus;
use clap::{Subcommand, ValueEnum};
use serde_json::json;
use std::path::PathBuf;

#[derive(Subcommand)]
pub(crate) enum InvCmd {
    /// Check that a file follows the investigation format.
    Validate { file: PathBuf },
    /// Resolve every anchor against a checkout; exits 1 if any are broken.
    Check {
        /// A file, or an https:// URL (fetched with a size limit and timeout).
        file: String,
        /// Root of the checkout.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Which code to check against: the working tree, or the investigation's pinned
        /// commit read from git.
        #[arg(long, value_enum, default_value_t = At::Worktree)]
        at: At,
        /// Don't use the cached semantic index; every edge stays `claimed`.
        #[arg(long)]
        no_index: bool,
    },
    /// List the investigations in `.chorong/investigations/` under a worktree.
    List {
        /// Worktree root.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum At {
    /// The files in the working tree.
    Worktree,
    /// The commit in the investigation's `rev`.
    Rev,
}

fn kind_str(kind: &Kind) -> &'static str {
    match kind {
        Kind::Callstack => "callstack",
        Kind::Tour => "tour",
        Kind::Diagram => "diagram",
        Kind::PrOverlay => "pr-overlay",
        Kind::Comparison => "comparison",
    }
}

pub(crate) fn run(cmd: InvCmd) -> Result<Output> {
    match cmd {
        InvCmd::Validate { file } => {
            let json = cho_core::fs::read_source_text(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            Ok(match Investigation::parse(&json) {
                Ok(inv) => Output::ok(
                    format!(
                        "valid: {} ({} nodes, {} edges)",
                        inv.title,
                        inv.nodes.len(),
                        inv.edges.len()
                    ),
                    json!({ "valid": true, "title": inv.title, "nodes": inv.nodes.len(), "edges": inv.edges.len() }),
                ),
                Err(e) => Output {
                    text: format!("invalid: {e}"),
                    json: json!({ "valid": false, "error": e.to_string() }),
                    ok: false,
                },
            })
        }
        InvCmd::Check {
            file,
            root,
            at,
            no_index,
        } => with_skill_note(&root.clone(), check(&file, root, at, no_index)),
        InvCmd::List { root } => with_skill_note(&root.clone(), list(root)),
    }
}

/// Says on stderr (and in the JSON) when the repository's copy of the investigation skill is
/// not this cho's, so agents writing investigations there follow the current rules.
fn with_skill_note(root: &std::path::Path, out: Result<Output>) -> Result<Output> {
    let mut out = out?;
    let outdated = crate::skill::outdated_under(root);
    if let Some(note) = crate::skill::outdated_note(&outdated) {
        eprintln!("{note}");
        if let Some(obj) = out.json.as_object_mut() {
            obj.insert("skill_outdated".into(), json!(outdated));
        }
    }
    Ok(out)
}

fn check(file: &str, root: PathBuf, at: At, no_index: bool) -> Result<Output> {
    let src = Source::from_arg(file)?;
    // A file named on the command line is the reader's own choice: no root confinement.
    let fetched = source::read(&src, None)?;
    let inv = source::parse_bytes(&fetched.bytes)?;
    let drift = drift(&root, &inv.rev);

    // The index describes the working tree, so it only helps when that is the code checked.
    let use_index = !no_index && (at == At::Worktree || drift.is_at_rev());
    let semantic = if use_index { open_index(&root)? } else { None };
    let oracle = semantic.as_ref().map(|s| s as &dyn EdgeOracle);
    let commit_reader;
    let worktree_reader = WorktreeReader { root: &root };
    let report = match at {
        At::Worktree => check_with_reader(&inv, &worktree_reader, oracle),
        At::Rev => {
            commit_reader = CommitReader::new(&root, &inv.rev)
                .map_err(|e| anyhow::anyhow!("{e}; fetch it, or check with `--at worktree`"))?;
            check_with_reader(&inv, &commit_reader, oracle)
        }
    };

    let mut text = String::new();
    if let Some(host) = fetched.final_url.as_deref().and_then(source::host_of) {
        text.push_str(&format!("from {host}\n"));
    }
    let short = |c: &str| c[..c.len().min(12)].to_string();
    match (&drift, at) {
        (Drift::AtRev { .. }, _) => {}
        (Drift::Differs { head, .. }, At::Worktree) => text.push_str(&format!(
            "note: the checkout is at {}, not the investigation's {}; checking the working tree (use `--at rev` for the pinned commit)\n",
            head.as_deref().map_or("no commit".into(), short),
            inv.rev
        )),
        (Drift::Differs { .. }, At::Rev) => {
            text.push_str(&format!("checking the pinned commit {}\n", inv.rev));
        }
        (Drift::NotGitRepo { .. }, _) => {
            text.push_str("note: not a git repository, so the pinned commit can't be compared\n");
        }
    }
    for n in &report.nodes {
        let at = n.line.map(|l| format!(" (line {l})")).unwrap_or_default();
        text.push_str(&format!(
            "{:<10} {}{at}\n",
            format!("{:?}", n.status).to_lowercase(),
            n.id
        ));
        for m in &n.messages {
            text.push_str(&format!("           {m}\n"));
        }
    }
    for e in &report.edges {
        text.push_str(&format!("{:<10} {} -> {}\n", e.status, e.from, e.to));
    }
    if let Some(v2) = &report.v2 {
        v2_text(&mut text, v2);
    }
    let broken = report.broken_total();
    let note = match &semantic {
        None if no_index => "Edges are `claimed` (--no-index).".to_string(),
        None if !use_index => {
            "Edges are `claimed`: the semantic index describes the working tree, not the pinned commit."
                .to_string()
        }
        None => "Edges are `claimed`: no semantic index (run `cho index`).".to_string(),
        Some(s) if s.status() == IndexStatus::Current => {
            "Edges confirmed by the semantic index are `verified`.".to_string()
        }
        Some(s) => format!(
            "Edges confirmed by the semantic index are `verified`; the index is {}, so results may be out of date (run `cho index`).",
            s.status()
        ),
    };
    let in_views = broken - report.broken_count();
    let also = if in_views > 0 {
        format!(", and {in_views} broken in views or the untraced list")
    } else {
        String::new()
    };
    text.push_str(&format!(
        "{} broken of {} nodes{also}. {note}\n",
        report.broken_count(),
        report.nodes.len()
    ));
    let mut json = serde_json::to_value(&report)?;
    json["index"] = match &semantic {
        Some(s) => json!({ "used": true, "status": s.status() }),
        None => json!({ "used": false }),
    };
    json["at"] = json!(match at {
        At::Worktree => "worktree",
        At::Rev => "rev",
    });
    json["drift"] = serde_json::to_value(&drift)?;
    let mut source_json = serde_json::to_value(&src)?;
    if let Some(host) = src.host() {
        source_json["host"] = json!(host);
    }
    if let Some(host) = fetched.final_url.as_deref().and_then(source::host_of) {
        source_json["final_host"] = json!(host);
    }
    json["source"] = source_json;
    Ok(Output {
        text,
        json,
        ok: broken == 0,
    })
}

/// The text lines for the version 2 parts of a check.
fn v2_text(text: &mut String, v2: &V2Report) {
    let status = |s: &NodeStatus| format!("{s:?}").to_lowercase();
    let anchor = |a: &AnchorReport| {
        let line = a.line.map(|l| format!(" line {l}")).unwrap_or_default();
        format!("{} ({}{line}, {})", a.label, a.path, status(&a.status))
    };
    for e in &v2.externals {
        text.push_str(&format!(
            "{:<10} {} ({}) not in the code\n",
            "external", e.id, e.label
        ));
    }
    for a in &v2.actions {
        text.push_str(&format!(
            "{:<10} {} \"{}\" ({} command(s) to copy; never run)\n",
            "action", a.id, a.name, a.commands
        ));
    }
    for s in &v2.steps {
        let mut line = format!("{:<10} {:<4} {}", "step", s.number, s.id);
        if let Some(n) = &s.name {
            line.push_str(&format!(" \"{n}\""));
        }
        if let Some(d) = &s.depth {
            line.push_str(&format!(
                " depth {} ({})",
                d.n,
                verification(d.verification)
            ));
        }
        if !s.called_by.is_empty() {
            let by: Vec<String> = s
                .called_by
                .iter()
                .map(|c| format!("{} ({})", c.id, verification(c.verification)))
                .collect();
            line.push_str(&format!(" called by {}", by.join(", ")));
        }
        if let Some(e) = s.author_evidence {
            line.push_str(&format!(" author: {}", evidence(e)));
        }
        if !s.values.is_empty() {
            line.push_str(&format!(" values: {}", s.values.join("; ")));
        }
        text.push_str(&line);
        text.push('\n');
    }
    for v in &v2.views {
        let panes: Vec<String> = v.panes.iter().map(pane_word).collect();
        let what = if v.fallback.is_some() {
            "default page"
        } else {
            v.arrangement.as_str()
        };
        text.push_str(&format!(
            "{:<10} {}: {what}: {}\n",
            "view",
            v.step,
            panes.join(", ")
        ));
    }
    for u in &v2.untraced {
        let at =
            u.at.as_ref()
                .map(|a| format!(" at {}", anchor(a)))
                .unwrap_or_default();
        text.push_str(&format!("{:<10} {}{at}\n", "untraced", u.what));
    }
    if let Some(c) = &v2.comparison {
        let sides: Vec<&str> = c.sides.iter().map(|s| s.id.as_str()).collect();
        text.push_str(&format!("{:<10} sides {}\n", "compare", sides.join(" | ")));
        for r in &c.rows {
            row_text(text, r, None);
        }
        for v in &c.variants {
            for r in &v.rows {
                row_text(text, r, Some(&format!("{}/{}", v.side, v.variant)));
            }
        }
    }
    for b in &v2.branches {
        let off: Vec<&str> = b
            .overrides
            .iter()
            .filter(|s| !s.reached)
            .map(|s| s.step.as_str())
            .collect();
        text.push_str(&format!(
            "{:<10} {}: {} step(s) changed, not reached: {}\n",
            "branch",
            b.id,
            b.overrides.len(),
            if off.is_empty() {
                "none".into()
            } else {
                off.join(", ")
            }
        ));
    }
    for m in &v2.messages {
        text.push_str(&format!("note: {m}\n"));
    }
}

fn row_text(text: &mut String, r: &RowReport, variant: Option<&str>) {
    let cells: Vec<String> = r
        .cells
        .iter()
        .map(|c| match (&c.step, &c.missing) {
            (Some(s), _) => match &c.anchor {
                Some(a) if a.status == NodeStatus::Broken => format!("{s} (broken)"),
                _ => s.clone(),
            },
            (None, Some(m)) => format!("missing: {m}"),
            (None, None) => "?".into(),
        })
        .collect();
    let result = match (r.result, r.result_source) {
        (Some(res), src) => format!("{} ({})", result_word(res), source_word(src)),
        (None, _) => "no result".into(),
    };
    let label = variant.map(|v| format!(" [{v}]")).unwrap_or_default();
    text.push_str(&format!(
        "{:<10} {:<4} {}{label}: {result}: {}\n",
        "row",
        r.number,
        r.id,
        cells.join(" | ")
    ));
}

fn pane_word(p: &PaneReport) -> String {
    match p {
        PaneReport::Editor { highlights, .. } if highlights.len() > 1 => {
            format!("editor ({} highlights)", highlights.len())
        }
        PaneReport::Editor { .. } => "editor".into(),
        PaneReport::Context { .. } => "context".into(),
        PaneReport::Outline { .. } => "outline".into(),
        PaneReport::Graph { .. } => "graph".into(),
        PaneReport::Callstack => "callstack".into(),
        PaneReport::Diff {
            path,
            base,
            head,
            problem,
        } => match problem {
            Some(p) => format!("diff {path} (unresolved: {p})"),
            None => format!("diff {path} ({} -> {})", base.words(), head.words()),
        },
        PaneReport::FileTree { .. } => "file-tree".into(),
    }
}

fn verification(v: Verification) -> &'static str {
    match v {
        Verification::Verified => "verified",
        Verification::Claimed => "claimed",
        Verification::Broken => "broken",
    }
}

fn evidence(e: Evidence) -> &'static str {
    e.word()
}

fn result_word(r: RowResult) -> &'static str {
    match r {
        RowResult::Same => "same",
        RowResult::Differs => "differs",
        RowResult::Missing => "missing",
    }
}

fn source_word(s: ResultSource) -> &'static str {
    match s {
        ResultSource::Derived => "derived",
        ResultSource::Claimed => "claimed",
        ResultSource::None => "none",
    }
}

fn list(root: PathBuf) -> Result<Output> {
    if !root.is_dir() {
        bail!("{} is not a folder", root.display());
    }
    let listing = listing::list(&root)
        .with_context(|| format!("cannot list investigations in {}", root.display()))?;
    let mut text = String::new();
    for e in &listing.entries {
        match &e.info {
            EntryInfo::Valid {
                title, kind, rev, ..
            } => text.push_str(&format!(
                "{:<11} {:<12} {}  {}\n",
                kind_str(kind),
                &rev[..rev.len().min(12)],
                e.path,
                title
            )),
            EntryInfo::Invalid { reason } => text.push_str(&format!(
                "{:<11} {:<12} {}  {reason}\n",
                "invalid", "", e.path
            )),
        }
    }
    if listing.entries.is_empty() {
        text.push_str(&format!(
            "no investigations in {}\n",
            root.join(listing::INVESTIGATIONS_DIR).display()
        ));
    }
    if listing.truncated {
        text.push_str(&format!(
            "only the first {} are listed\n",
            listing::MAX_LISTED
        ));
    }
    let mut json = serde_json::to_value(&listing)?;
    json["root"] = json!(root.display().to_string());
    Ok(Output::ok(text, json))
}
