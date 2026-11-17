//! `cho changes`: the files and declarations that differ between two commits, read from the
//! object store (`cho-changes`). See `docs/public/cli.md` ("Changes output").

use crate::Output;
use anyhow::{Result, anyhow};
use cho_changes::{
    ChangeSet, Decl, FileStatus, SymbolStatus, compare, compare_with_merge_base, compare_worktree,
    compare_worktree_with_merge_base,
};
use cho_git::short;
use clap::Args;
use std::fmt::Write;
use std::path::PathBuf;

#[derive(Args)]
pub struct ChangesArgs {
    /// Before: any revision naming a commit.
    #[arg(long, group = "before", value_name = "REV")]
    base: Option<String>,
    /// Before is the merge base of `--head` and TARGET (computed locally).
    #[arg(long, group = "before", value_name = "TARGET")]
    merge_base: Option<String>,
    /// After.
    #[arg(long, default_value = "HEAD", value_name = "REV")]
    head: String,
    /// After is the working folder (HEAD plus uncommitted and untracked files), not a commit;
    /// hunks and declarations touched by uncommitted work are marked `uncommitted`.
    #[arg(long, conflicts_with = "head")]
    worktree: bool,
    /// Repository root (any folder inside the repository).
    #[arg(long, default_value = ".")]
    root: PathBuf,
}

pub fn run(a: ChangesArgs) -> Result<Output> {
    if !a.root.is_dir() {
        return Err(anyhow!("{} is not a folder", a.root.display()));
    }
    let set = match (&a.base, &a.merge_base, a.worktree) {
        (Some(base), _, false) => compare(&a.root, base, &a.head),
        (None, Some(target), false) => compare_with_merge_base(&a.root, target, &a.head),
        (Some(base), _, true) => compare_worktree(&a.root, base),
        (None, Some(target), true) => compare_worktree_with_merge_base(&a.root, target),
        (None, None, _) => return Err(anyhow!("give --base REV or --merge-base TARGET")),
    }
    .map_err(|e| anyhow!(e))?;
    Ok(Output::ok(text(&set), serde_json::to_value(&set)?))
}

fn lines(d: &[Decl]) -> String {
    d.iter()
        .map(|d| format!("{}-{}", d.start_line, d.end_line))
        .collect::<Vec<_>>()
        .join(", ")
}

fn text(set: &ChangeSet) -> String {
    let mut t = String::new();
    match &set.merge_base {
        Some(mb) => {
            let _ = writeln!(
                t,
                "base {} (merge base with {})",
                short(&set.base),
                mb.target
            );
        }
        None => {
            let _ = writeln!(t, "base {}", short(&set.base));
        }
    }
    if set.worktree {
        let _ = writeln!(t, "head {} plus the working folder", short(&set.head));
    } else {
        let _ = writeln!(t, "head {}", short(&set.head));
    }
    for n in &set.notes {
        let _ = writeln!(t, "note: {n}");
    }
    if set.files.is_empty() {
        t.push_str("no changes\n");
    }
    for f in &set.files {
        let mark = match f.status {
            FileStatus::Added => "A",
            FileStatus::Deleted => "D",
            FileStatus::Renamed => "R",
            FileStatus::Modified => "M",
        };
        let mut line = match (&f.old_path, f.similarity) {
            (Some(old), Some(s)) => format!("{mark} {old} -> {} ({s}%)", f.path),
            _ => format!("{mark} {}", f.path),
        };
        if let Some(r) = f.level.reason() {
            let _ = write!(line, "  [file level: {}]", r.as_str());
        }
        if f.coarse {
            line.push_str("  [coarse diff]");
        }
        if f.uncommitted {
            let n = f.hunks.iter().filter(|h| h.uncommitted).count();
            let _ = write!(line, "  [uncommitted: {n} of {} hunks]", f.hunks.len());
        }
        let _ = writeln!(t, "{line}");
        for s in &f.symbols {
            let (mark, at) = match s.status {
                SymbolStatus::Added => ("+", format!("head {}", lines(&s.head))),
                SymbolStatus::Removed => ("-", format!("base {}", lines(&s.base))),
                SymbolStatus::Changed => (
                    "~",
                    format!("base {} -> head {}", lines(&s.base), lines(&s.head)),
                ),
                SymbolStatus::Ambiguous => (
                    "?",
                    format!(
                        "ambiguous: {} at base ({}), {} at head ({})",
                        s.base.len(),
                        lines(&s.base),
                        s.head.len(),
                        lines(&s.head)
                    ),
                ),
            };
            let flag = if s.uncommitted { "  [uncommitted]" } else { "" };
            let _ = writeln!(
                t,
                "  {mark} {} {}  {at}{flag}",
                s.kind.as_str(),
                s.qualified_name
            );
        }
        if f.outside_declarations {
            t.push_str("  * changes outside declarations\n");
        }
    }
    t.trim_end().to_string()
}
